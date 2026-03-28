use std::collections::HashMap;
use std::mem;
use std::ptr;
use std::sync::{Arc, Mutex, RwLock, atomic::{AtomicUsize, Ordering}};

#[repr(C)]
pub struct ListHead {
    pub next: *mut ListHead,
    pub prev: *mut ListHead,
}

impl ListHead {
    pub fn new() -> ListHead {
        let mut head = ListHead {
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        };
        head.init();
        head
    }

    pub fn init(&mut self) {
        self.next = self;
        self.prev = self;
    }

    pub fn is_empty(&self) -> bool {
        self.next == self
    }

    pub unsafe fn list_add(new: *mut ListHead, head: *mut ListHead) {
        let next = (*head).next;
        (*next).prev = new;
        (*new).next = next;
        (*new).prev = head;
        (*head).next = new;
    }

    pub unsafe fn list_del(entry: *mut ListHead) {
        let prev = (*entry).prev;
        let next = (*entry).next;
        (*next).prev = prev;
        (*prev).next = next;
        // Optional poisoning
        (*entry).next = ptr::null_mut();
        (*entry).prev = ptr::null_mut();
    }
}

#[repr(C)]
pub struct DeathNotification {
    pub cookie: usize,
    pub list: ListHead,
}

pub struct BinderNode {
    pub ptr: usize,
    handle: u32,
    refs: AtomicUsize,
    death_list: Mutex<ListHead>,
}

impl BinderNode {
    pub fn new(ptr: usize, handle: u32) -> Self {
        Self {
            ptr,
            handle,
            refs: AtomicUsize::new(1),
            death_list: Mutex::new(ListHead::new()),
        }
    }

    pub fn inc_ref(&self) {
        self.refs.fetch_add(1, Ordering::Release);
    }

    pub fn dec_ref(&self) -> bool {
        self.refs.fetch_sub(1, Ordering::AcqRel) == 1
    }

    pub fn add_death_notification(&self, cookie: usize) {
        let raw_dn: *mut DeathNotification = Box::into_raw(Box::new(DeathNotification {
            cookie,
            list: ListHead::new(),
        }));
        let list_offset = mem::offset_of!(DeathNotification, list);
        let list_ptr = unsafe { (raw_dn as *mut u8).add(list_offset) as *mut ListHead };
        let mut guard = self.death_list.lock().unwrap();
        let head_ptr = &mut *guard as *mut ListHead;
        unsafe { ListHead::list_add(list_ptr, head_ptr) };
    }
}

pub struct BinderManager {
    nodes: RwLock<HashMap<u32, *mut BinderNode>>,
    cleanup_queue: Mutex<Vec<*mut BinderNode>>,
}

unsafe impl Send for BinderNode {}
unsafe impl Sync for BinderNode {}

impl BinderManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            nodes: RwLock::new(HashMap::new()),
            cleanup_queue: Mutex::new(Vec::new()),
        })
    }

    pub fn create_node(&self, handle: u32, ptr: usize) -> *mut BinderNode {
        let node = Box::new(BinderNode::new(ptr, handle));
        let nptr = Box::into_raw(node);
        self.nodes.write().unwrap().insert(handle, nptr);
        nptr
    }

    pub fn acquire_node(&self, handle: u32) -> Option<*mut BinderNode> {
        let nptr = {
            let guard = self.nodes.read().unwrap();
            guard.get(&handle).copied().unwrap_or(ptr::null_mut())
        };
        if nptr.is_null() {
            return None;
        }
        let node = unsafe { &*nptr };
        if node.refs.load(Ordering::Acquire) == 0 {
            return None;
        }
        node.inc_ref();
        Some(nptr)
    }

    pub fn release_ref(&self, nptr: *mut BinderNode) {
        let node = unsafe { &*nptr };
        if node.dec_ref() {
            // Remove from map before enqueuing
            self.nodes.write().unwrap().remove(&node.handle);
            let mut queue = self.cleanup_queue.lock().unwrap();
            queue.push(nptr);
        }
    }

    pub fn release_handle(&self, handle: u32) {
        let nptr_opt = {
            let mut guard = self.nodes.write().unwrap();
            guard.remove(&handle)
        };
        if let Some(nptr) = nptr_opt {
            self.release_ref(nptr);
        }
    }

    pub fn process_cleanup(&self) {
        let batch: Vec<*mut BinderNode>;
        {
            let mut queue = self.cleanup_queue.lock().unwrap();
            batch = mem::take(&mut *queue);
        }
        for nptr in batch {
            let node = unsafe { &*nptr };
            if node.refs.load(Ordering::Acquire) != 0 {
                continue; // Revived (unlikely in this design)
            }
            // Process death notifications
            let mut death_guard = node.death_list.lock().unwrap();
            let list_offset = mem::offset_of!(DeathNotification, list);
            while !death_guard.is_empty() {
                let entry_ptr = death_guard.next;
                unsafe { ListHead::list_del(entry_ptr) };
                let dn_ptr = unsafe {
                    (entry_ptr as *mut u8).sub(list_offset) as *mut DeathNotification
                };
                // Simplified: no callback, just cleanup
                // println!("Death notification cookie: {}", unsafe { (*dn_ptr).cookie });
                unsafe { drop(Box::from_raw(dn_ptr)) };
            }
            drop(death_guard);
            // Free the node
            unsafe { drop(Box::from_raw(nptr)) };
        }
    }
}
