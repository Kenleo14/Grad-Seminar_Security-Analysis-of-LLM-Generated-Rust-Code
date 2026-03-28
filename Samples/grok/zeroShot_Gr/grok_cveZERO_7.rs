use std::collections::HashMap;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, RwLock};

#[repr(C)]
pub struct DeathNotification {
    pub cookie: usize,
    next: *mut DeathNotification,
}

pub struct BinderNode {
    pub node_id: u64,
    strong_refs: AtomicUsize,
    weak_refs: AtomicUsize,
    flags: AtomicU32,
    death_head: AtomicPtr<DeathNotification>,
}

#[derive(Default)]
struct ManagerInner {
    next_id: u64,
    nodes: HashMap<u64, *mut BinderNode>,
}

pub struct BinderNodeManager {
    inner: RwLock<ManagerInner>,
    cleanup_queue: Mutex<Vec<*mut BinderNode>>,
}

impl BinderNodeManager {
    /// Creates a new BinderNodeManager.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(ManagerInner::default()),
            cleanup_queue: Mutex::new(Vec::new()),
        }
    }

    /// Creates a new binder node and returns its handle (ID).
    pub fn create_node(&self) -> u64 {
        let mut inner = self.inner.write().unwrap();
        let id = inner.next_id;
        inner.next_id = inner.next_id.wrapping_add(1);
        let node_box = Box::new(BinderNode {
            node_id: id,
            strong_refs: AtomicUsize::new(1),
            weak_refs: AtomicUsize::new(0),
            flags: AtomicU32::new(0),
            death_head: AtomicPtr::new(ptr::null_mut()),
        });
        let node_ptr = Box::into_raw(node_box);
        inner.nodes.insert(id, node_ptr);
        id
    }

    /// Increments the strong reference count for the node identified by `handle`.
    /// Returns `true` if the node existed and the reference was incremented.
    pub fn inc_strong_ref(&self, handle: u64) -> bool {
        let node_ptr = {
            let inner = self.inner.read().unwrap();
            *inner.nodes.get(&handle).unwrap_or(&ptr::null_mut())
        };
        if node_ptr.is_null() {
            return false;
        }
        unsafe {
            let prev = (*node_ptr).strong_refs.fetch_add(1, Ordering::AcqRel);
            if prev == 0 {
                (*node_ptr).strong_refs.fetch_sub(1, Ordering::Relaxed);
                return false;
            }
        }
        true
    }

    /// Decrements the strong reference count for the node identified by `handle`.
    /// If the count reaches zero, the node is queued for cleanup.
    /// Returns `true` if the node existed.
    pub fn dec_strong_ref(&self, handle: u64) -> bool {
        let node_ptr = {
            let inner = self.inner.read().unwrap();
            *inner.nodes.get(&handle).unwrap_or(&ptr::null_mut())
        };
        if node_ptr.is_null() {
            return false;
        }
        unsafe {
            loop {
                let count = (*node_ptr).strong_refs.load(Ordering::Acquire);
                if count == 0 {
                    return false;
                }
                match (*node_ptr).strong_refs.compare_exchange(
                    count,
                    count - 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        if count == 1 {
                            let mut inner = self.inner.write().unwrap();
                            if let Some(&ptr) = inner.nodes.get(&handle) {
                                let curr = (*ptr).strong_refs.load(Ordering::Acquire);
                                if curr == 0 {
                                    inner.nodes.remove(&handle);
                                    self.cleanup_queue.lock().unwrap().push(ptr);
                                }
                            }
                        }
                        return true;
                    }
                    Err(_) => continue,
                }
            }
        }
    }

    /// Adds a death notification to the node identified by `handle`.
    /// Increments the weak reference count.
    /// Returns `true` if successfully added.
    pub fn add_death_notification(&self, handle: u64, cookie: usize) -> bool {
        let node_ptr = {
            let inner = self.inner.read().unwrap();
            *inner.nodes.get(&handle).unwrap_or(&ptr::null_mut())
        };
        if node_ptr.is_null() {
            return false;
        }
        unsafe {
            let strong = (*node_ptr).strong_refs.load(Ordering::Relaxed);
            if strong == 0 {
                return false;
            }
            (*node_ptr).weak_refs.fetch_add(1, Ordering::Relaxed);
        }
        let mut dn_box = Box::new(DeathNotification {
            cookie,
            next: ptr::null_mut(),
        });
        let dn_ptr = Box::into_raw(dn_box);
        unsafe {
            loop {
                let head = (*node_ptr).death_head.load(Ordering::Relaxed);
                (*dn_ptr).next = head;
                if (*node_ptr)
                    .death_head
                    .compare_exchange(head, dn_ptr, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    return true;
                }
            }
        }
    }

    /// Processes the temporary cleanup queue.
    /// Notifies all death recipients and frees nodes if no remaining weak references.
    pub fn process_cleanup(&self) {
        let mut queue = self.cleanup_queue.lock().unwrap();
        let dead_nodes = std::mem::take(&mut *queue);
        drop(queue);
        for node_ptr in dead_nodes {
            unsafe {
                let mut head = (*node_ptr).death_head.swap(ptr::null_mut(), Ordering::AcqRel);
                while !head.is_null() {
                    let cookie = (*head).cookie;
                    let next = (*head).next;
                    println!(
                        "Death notification: node_id={}, cookie={}",
                        (*node_ptr).node_id, cookie
                    );
                    (*node_ptr).weak_refs.fetch_sub(1, Ordering::Release);
                    ptr::drop_in_place(head);
                    head = next;
                }
                // Free the node if no remaining weak refs
                let remaining_weak = (*node_ptr).weak_refs.load(Ordering::Acquire);
                if remaining_weak == 0 {
                    let _ = Box::from_raw(node_ptr);
                } else {
                    eprintln!(
                        "Warning: node {} has {} remaining weak refs on cleanup",
                        (*node_ptr).node_id, remaining_weak
                    );
                    let _ = Box::from_raw(node_ptr);
                }
            }
        }
    }
}
