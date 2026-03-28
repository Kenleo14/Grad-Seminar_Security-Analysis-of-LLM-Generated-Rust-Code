use std::cell::UnsafeCell;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::{self, Acquire, AcqRel, Release}};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct BinderNodeManager(Arc<ManagerInner>);

struct ManagerInner {
    temp_list: Mutex<Vec<NonNull<BinderNode>>>,
}

impl BinderNodeManager {
    pub fn new() -> Self {
        Self(Arc::new(ManagerInner {
            temp_list: Mutex::new(Vec::new()),
        }))
    }

    pub fn new_node(&self) -> StrongHandle {
        let mut node = Box::new(BinderNode::new());
        let np = NonNull::new(Box::into_raw(node)).unwrap();
        unsafe {
            np.as_ref().strong_count.store(1, Release);
        }
        StrongHandle {
            node: np,
            manager: self.0.clone(),
        }
    }

    pub fn new_weak_handle(&self, strong: &StrongHandle) -> WeakHandle {
        unsafe {
            strong.node.as_ref().inc_weak();
        }
        WeakHandle {
            node: strong.node,
            manager: self.0.clone(),
        }
    }

    pub fn process_cleanup(&self) {
        let mut temp = self.0.temp_list.lock().unwrap();
        temp.retain_mut(|np| {
            let node_ref = unsafe { np.as_ref() };
            let s = node_ref.strong_count.load(Acquire);
            let w = node_ref.weak_count.load(Acquire);
            if s == 0 && w == 0 {
                drop(unsafe { Box::from_raw(np.as_ptr()) });
                false
            } else {
                true
            }
        });
    }
}

pub struct StrongHandle {
    node: NonNull<BinderNode>,
    manager: Arc<BinderNodeManager>,
}

impl StrongHandle {
    pub fn node(&self) -> &BinderNode {
        unsafe { self.node.as_ref() }
    }

    pub fn upgrade_to_weak(&self, mgr: &BinderNodeManager) -> WeakHandle {
        mgr.new_weak_handle(self)
    }
}

impl Drop for StrongHandle {
    fn drop(&mut self) {
        unsafe {
            self.node.as_ref().dec_strong(&*self.manager.0);
        }
    }
}

pub struct WeakHandle {
    node: NonNull<BinderNode>,
    manager: Arc<BinderNodeManager>,
}

impl WeakHandle {
    pub fn node(&self) -> &BinderNode {
        unsafe { self.node.as_ref() }
    }
}

impl Drop for WeakHandle {
    fn drop(&mut self) {
        unsafe {
            self.node.as_ref().dec_weak(&*self.manager.0);
        }
    }
}

pub struct DeathRecipient {
    pub cookie: usize,
    next: UnsafeCell<*mut DeathRecipient>,
    prev: UnsafeCell<*mut DeathRecipient>,
}

impl DeathRecipient {
    pub fn new(cookie: usize) -> Self {
        Self {
            cookie,
            next: UnsafeCell::new(ptr::null_mut()),
            prev: UnsafeCell::new(ptr::null_mut()),
        }
    }
}

pub struct BinderNode {
    strong_count: AtomicUsize,
    weak_count: AtomicUsize,
    is_dead: AtomicBool,
    queued: AtomicBool,
    death_lock: Mutex<()>,
    death_head: UnsafeCell<*mut DeathRecipient>,
    death_tail: UnsafeCell<*mut DeathRecipient>,
}

impl BinderNode {
    fn new() -> Self {
        Self {
            strong_count: AtomicUsize::new(0),
            weak_count: AtomicUsize::new(0),
            is_dead: AtomicBool::new(false),
            queued: AtomicBool::new(false),
            death_lock: Mutex::new(()),
            death_head: UnsafeCell::new(ptr::null_mut()),
            death_tail: UnsafeCell::new(ptr::null_mut()),
        }
    }

    pub fn inc_strong(&self) {
        self.strong_count.fetch_add(1, Acquire);
    }

    pub fn inc_weak(&self) {
        self.weak_count.fetch_add(1, Acquire);
    }

    fn dec_strong(&self, mgr: &ManagerInner) {
        let prev = self.strong_count.fetch_sub(1, Release);
        if prev == 1 {
            if self.is_dead.swap(true, AcqRel) == false {
                self.notify_deaths();
            }
            self.schedule_for_cleanup(mgr);
        }
    }

    fn dec_weak(&self, mgr: &ManagerInner) {
        let prev = self.weak_count.fetch_sub(1, Release);
        if prev == 1 {
            self.schedule_for_cleanup(mgr);
        }
    }

    fn schedule_for_cleanup(&self, mgr: &ManagerInner) {
        if self.queued.swap(true, AcqRel) == false {
            let ptr = self as *const Self as *mut Self;
            let np = NonNull::new(ptr).unwrap();
            let mut temp_list = mgr.temp_list.lock().unwrap();
            temp_list.push(np);
        }
    }

    fn notify_deaths(&self) {
        let mut cookies = Vec::new();
        {
            let _guard = self.death_lock.lock().unwrap();
            let mut current = unsafe { *self.death_head.get() };
            while !current.is_null() {
                let recip = unsafe { &*current };
                cookies.push(recip.cookie);
                current = unsafe { *recip.next.get() };
            }
            unsafe {
                *self.death_head.get() = ptr::null_mut();
                *self.death_tail.get() = ptr::null_mut();
            }
        }
        // Notify callbacks outside the lock (dummy implementation)
        for cookie in cookies {
            // extern fn callback(cookie: usize); callback(cookie);
            // For demo: do nothing or log
        }
    }

    pub fn add_death_recipient(&self, recip_ptr: NonNull<DeathRecipient>) {
        let _guard = self.death_lock.lock().unwrap();
        unsafe {
            self.list_push_back(recip_ptr.as_ptr());
        }
    }

    pub fn remove_death_recipient(&self, recip_ptr: NonNull<DeathRecipient>) {
        let _guard = self.death_lock.lock().unwrap();
        unsafe {
            self.list_erase(recip_ptr.as_ptr());
        }
    }

    unsafe fn list_push_back(&self, entry: *mut DeathRecipient) {
        let headp = self.death_head.get();
        let tailp = self.death_tail.get();
        let tail = *tailp;

        // Clear entry links
        *(*entry).prev.get() = ptr::null_mut();
        *(*entry).next.get() = ptr::null_mut();

        if tail.is_null() {
            *headp = entry;
            *tailp = entry;
        } else {
            *(*tail).next.get() = entry;
            *(*entry).prev.get() = tail;
            *tailp = entry;
        }
    }

    unsafe fn list_erase(&self, entry: *mut DeathRecipient) {
        let headp = self.death_head.get();
        let tailp = self.death_tail.get();
        let prev_entry = *(*entry).prev.get();
        let next_entry = *(*entry).next.get();

        if prev_entry.is_null() {
            *headp = next_entry;
        } else {
            *(*prev_entry).next.get() = next_entry;
        }

        if next_entry.is_null() {
            *tailp = prev_entry;
        } else {
            *(*next_entry).prev.get() = prev_entry;
        }

        // Clear entry links
        *(*entry).prev.get() = ptr::null_mut();
        *(*entry).next.get() = ptr::null_mut();
    }
}
