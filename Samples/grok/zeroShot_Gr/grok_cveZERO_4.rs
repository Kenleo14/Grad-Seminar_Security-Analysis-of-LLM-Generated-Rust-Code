use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr;
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}, Mutex, RwLock};
use std::thread_local;

thread_local! {
    static LOCAL_TEMP_NODES: RefCell<Vec<Arc<BinderNode>>> = RefCell::new(Vec::new());
}

/// Intrusive doubly-linked list links, similar to Linux kernel list_head.
///
/// Must be the first field in the containing struct for container_of to work.
#[repr(C)]
pub struct ListLinks {
    prev: *mut ListLinks,
    next: *mut ListLinks,
}

impl ListLinks {
    fn null() -> Self {
        Self {
            prev: ptr::null_mut(),
            next: ptr::null_mut(),
        }
    }
}

/// Sentinel list head for the intrusive list.
pub struct ListHead {
    links: ListLinks,
}

impl ListHead {
    pub fn new() -> Self {
        Self {
            links: ListLinks::null(),
        }
    }

    /// Initialize the list head as empty (circular self-references).
    pub unsafe fn init(&mut self) {
        let head_ptr = &mut self.links as *mut ListLinks;
        self.links.prev = head_ptr;
        self.links.next = head_ptr;
    }

    /// Check if the list is empty.
    pub unsafe fn is_empty(&self) -> bool {
        let head_ptr = &self.links as *const ListLinks as *mut ListLinks;
        head_ptr == self.links.next
    }

    /// Insert a new entry after the given `prev` entry.
    pub unsafe fn add_after(&mut self, prev_ptr: *mut ListLinks, new_ptr: *mut ListLinks) {
        let next_ptr = (*prev_ptr).next;
        (*new_ptr).next = next_ptr;
        (*new_ptr).prev = prev_ptr;
        (*next_ptr).prev = new_ptr;
        (*prev_ptr).next = new_ptr;
    }

    /// Insert a new entry at the head of the list.
    pub unsafe fn add_head(&mut self, new_ptr: *mut ListLinks) {
        self.add_after(&mut self.links, new_ptr);
    }

    /// Remove an entry from the list.
    pub unsafe fn delete(&mut self, entry_ptr: *mut ListLinks) {
        let prev_ptr = (*entry_ptr).prev;
        let next_ptr = (*entry_ptr).next;
        (*next_ptr).prev = prev_ptr;
        (*prev_ptr).next = next_ptr;
    }

    /// Iterate over all entries in the list.
    pub unsafe fn for_each<F>(&self, mut callback: F)
    where
        F: FnMut(*mut DeathNotification),
    {
        let mut pos = self.links.next;
        let head_ptr = &self.links as *const _ as *mut ListLinks;
        while pos != head_ptr {
            let dn_ptr = container_of(pos, DeathNotification::OFFSET_LINKS);
            callback(dn_ptr);
            pos = (*pos).next;
        }
    }
}

/// Compute the container pointer from the ListLinks field pointer.
unsafe fn container_of(links_ptr: *mut ListLinks, offset: isize) -> *mut DeathNotification {
    ((links_ptr as *mut u8).offset(-offset)) as *mut DeathNotification
}

impl DeathNotification {
    /// Offset of `links` field for container_of.
    const OFFSET_LINKS: isize = std::mem::offset_of!(DeathNotification, links) as isize;
}

/// A death notification entry tracked in an intrusive list.
#[derive(Debug)]
pub struct DeathNotification {
    pub cookie: usize,
    pub recipient: String,  // Placeholder for actual recipient/callback
    links: ListLinks,
}

impl DeathNotification {
    pub fn new(cookie: usize, recipient: String) -> Box<Self> {
        Box::new(Self {
            cookie,
            recipient,
            links: ListLinks::null(),
        })
    }
}

/// A simplified BinderNode.
#[derive(Debug)]
pub struct BinderNode {
    pub id: u64,
    strong_refs: AtomicUsize,
    death_recipient_list: Mutex<ListHead>,
}

impl BinderNode {
    pub fn new(id: u64) -> Self {
        let mut head = ListHead::new();
        unsafe { head.init() };
        Self {
            id,
            strong_refs: AtomicUsize::new(0),
            death_recipient_list: Mutex::new(head),
        }
    }

    /// Increment the strong reference count.
    pub fn inc_strong(&self) {
        self.strong_refs.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement the strong reference count. Returns true if this was the last reference.
    pub fn dec_strong(&self) -> bool {
        let prev = self.strong_refs.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            self.death_notify();
            true
        } else {
            false
        }
    }

    /// Notify all death recipients (simplified: prints to stdout).
    fn death_notify(&self) {
        let guard = self.death_recipient_list.lock().unwrap();
        let list = &*guard;
        unsafe {
            list.for_each(|dn_ptr| {
                let dn = &*dn_ptr;
                println!(
                    "BinderNode {} death notification: cookie={}, recipient={}",
                    self.id, dn.cookie, dn.recipient
                );
            });
        }
        // Optionally unlink all here, but omitted for simplicity (clients should unlink).
    }

    /// Link a death notification into the node's list (caller owns the box).
    pub fn link_death_recipient(&self, dn: &mut ListLinks) {
        let mut guard = self.death_recipient_list.lock().unwrap();
        unsafe {
            (*guard).add_head(dn as *mut _);
        }
    }

    /// Unlink a death notification from the node's list.
    pub fn unlink_death_recipient(&self, dn: &mut ListLinks) {
        let mut guard = self.death_recipient_list.lock().unwrap();
        unsafe {
            (*guard).delete(dn as *mut _);
        }
    }
}

/// Simplified Android Binder Node management system.
#[derive(Clone)]
pub struct BinderNodeManager {
    nodes: Arc<RwLock<HashMap<u64, Weak<BinderNode>>>>,
    cleanup_queue: Arc<Mutex<Vec<Arc<BinderNode>>>>,
}

impl BinderNodeManager {
    /// Create a new manager.
    pub fn new() -> Self {
        Self {
            nodes: Arc::new(RwLock::new(HashMap::new())),
            cleanup_queue: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Create a new node and register it.
    pub fn create_node(&self, id: u64) -> Arc<BinderNode> {
        let node = Arc::new(BinderNode::new(id));
        let weak_node = Arc::downgrade(&node);
        self.nodes.write().unwrap().insert(id, weak_node);
        node
    }

    /// Acquire a strong reference to a node by ID (increments logical strong ref).
    /// Drains local temp list first to ensure stability.
    pub fn acquire_node(&self, id: u64) -> Option<Arc<BinderNode>> {
        self.drain_local_temp();
        let nodes_guard = self.nodes.read().unwrap();
        let weak_node = match nodes_guard.get(&id) {
            Some(weak) => weak.clone(),
            None => return None,
        };
        drop(nodes_guard);
        weak_node.upgrade().map(|node| {
            node.inc_strong();
            node
        })
    }

    /// Schedule a node for cleanup (move to local temp list).
    pub fn schedule_cleanup(&self, node: Arc<BinderNode>) {
        LOCAL_TEMP_NODES.with(|local| {
            local.borrow_mut().push(node);
        });
    }

    /// Drain thread-local temp nodes to global cleanup queue.
    fn drain_local_temp(&self) {
        LOCAL_TEMP_NODES.with(|local| {
            let mut local_nodes = local.borrow_mut();
            if !local_nodes.is_empty() {
                let mut queue = self.cleanup_queue.lock().unwrap();
                queue.append(&mut local_nodes);
            }
        });
    }

    /// Process all pending cleanups: drain locals and remove nodes from registry.
    pub fn process_cleanups(&self) {
        self.drain_local_temp();
        let mut queue = self.cleanup_queue.lock().unwrap();
        let mut nodes_guard = self.nodes.write().unwrap();
        queue.retain(|node| {
            // Remove from registry (drops weak ref).
            if nodes_guard.remove(&node.id).is_some() {
                // Node entry cleaned up.
                false
            } else {
                // Already removed (racy concurrent cleanup).
                true
            }
        });
    }
}
