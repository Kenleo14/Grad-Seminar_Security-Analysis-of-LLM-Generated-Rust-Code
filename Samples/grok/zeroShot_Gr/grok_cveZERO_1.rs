use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::ptr;

#[repr(C)]
struct NodeLinks {
    next: AtomicPtr<NodeLinks>,
}

#[derive(Debug)]
pub struct DeathNotification {
    pub cookie: u64,
    links: NodeLinks,
}

impl DeathNotification {
    pub fn new(cookie: u64) -> Arc<Self> {
        Arc::new(Self {
            cookie,
            links: NodeLinks {
                next: AtomicPtr::new(ptr::null_mut()),
            },
        })
    }
}

struct ListHead {
    head: *mut NodeLinks,
}

unsafe impl Send for ListHead {}
unsafe impl Sync for ListHead {}

pub struct BinderNode {
    id: u64,
    refs: AtomicUsize,
    dead: AtomicBool,
    death_list: Mutex<ListHead>,
    manager: Weak<BinderNodeManager>,
}

impl BinderNode {
    fn new(id: u64, manager: Weak<BinderNodeManager>) -> Self {
        Self {
            id,
            refs: AtomicUsize::new(1),
            dead: AtomicBool::new(false),
            death_list: Mutex::new(ListHead {
                head: ptr::null_mut(),
            }),
            manager,
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// Attempts to increment the reference count. Returns true if successful (node not dead).
    pub fn inc_ref(&self) -> bool {
        if self.dead.load(Ordering::Acquire) {
            false
        } else {
            self.refs.fetch_add(1, Ordering::Relaxed);
            true
        }
    }

    /// Decrements the reference count. If this was the last reference, marks the node as dead
    /// and queues it for cleanup via the manager.
    pub fn dec_ref(&self) {
        if self.refs.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.dead.store(true, Ordering::Release);
            if let Some(manager) = self.manager.upgrade() {
                let mut cleanup = manager.cleanup_list.lock().unwrap();
                cleanup.push(self.id);
            }
        }
    }

    /// Registers a death notification recipient. Returns true if registered (node not dead).
    /// The Arc<DeathNotification> must outlive the registration or until notified.
    pub fn register_death(&self, recip: &Arc<DeathNotification>) -> bool {
        if self.dead.load(Ordering::Acquire) {
            return false;
        }
        let links_ptr = Arc::as_ptr(recip) as *mut NodeLinks;
        // Initialize the recipient's next pointer.
        unsafe {
            (*links_ptr).next.store(ptr::null_mut(), Ordering::Relaxed);
        }
        let mut guard = self.death_list.lock().unwrap();
        let old_head = guard.head;
        unsafe {
            (*links_ptr).next.store(old_head, Ordering::Release);
        }
        guard.head = links_ptr;
        true
    }

    /// Notifies all registered death recipients and clears the list.
    /// Intended to be called during cleanup.
    pub fn notify_deaths(&self) {
        let mut guard = self.death_list.lock().unwrap();
        let mut cur_head = guard.head;
        guard.head = ptr::null_mut();
        drop(guard);
        while !cur_head.is_null() {
            unsafe {
                let death_node = cur_head as *mut DeathNotification;
                let cookie = (*death_node).cookie;
                // Simulate death notification callback.
                // In a real system, invoke user-provided callback.
                eprintln!("BinderNode({}) death notification: cookie={}", self.id, cookie);
                cur_head = (*cur_head).next.load(Ordering::Acquire);
            }
        }
    }
}

pub struct BinderNodeManager {
    nodes: RwLock<HashMap<u64, Arc<BinderNode>>>,
    cleanup_list: Mutex<Vec<u64>>,
}

impl BinderNodeManager {
    /// Creates a new manager.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            nodes: RwLock::new(HashMap::new()),
            cleanup_list: Mutex::new(Vec::new()),
        })
    }

    /// Creates a new binder node with the given ID.
    pub fn create_node(&self, id: u64) -> Arc<BinderNode> {
        let manager_weak = Arc::downgrade(self);
        let node = Arc::new(BinderNode::new(id, manager_weak));
        self.nodes.write().unwrap().insert(id, node.clone());
        node
    }

    /// Acquires a reference to a binder node by ID. Returns None if not found or dead.
    pub fn get_node(&self, id: u64) -> Option<Arc<BinderNode>> {
        let guard = self.nodes.read().unwrap();
        guard.get(&id).and_then(|node| {
            if node.dead.load(Ordering::Acquire) {
                None
            } else {
                Some(node.clone())
            }
        })
    }

    /// Processes queued cleanup requests: removes dead nodes from the map and notifies deaths.
    /// Should be called periodically or by a background thread.
    pub fn process_cleanup(&self) {
        let ids_to_clean = {
            let mut list = self.cleanup_list.lock().unwrap();
            std::mem::take(&mut *list)
        };
        let mut nodes_guard = self.nodes.write().unwrap();
        for id in ids_to_clean {
            if let Some(node) = nodes_guard.remove(&id) {
                // Notify deaths while holding the write lock (safe, as death_list lock is per-node).
                node.notify_deaths();
            }
        }
    }
}
