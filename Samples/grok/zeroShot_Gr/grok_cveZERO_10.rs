use std::sync::{Arc, Mutex, AtomicUsize, atomic::{AtomicPtr, Ordering}};
use std::ptr::{NonNull, null_mut};
use std::cell::UnsafeCell;
use std::mem;

// Core trait for death recipients
pub trait DeathRecipient: Send + Sync {
    fn notify(&self, cookie: u64);
}

// Intrusive link structure for doubly-linked lists (POD-like for safety)
#[repr(C)]
#[derive(Default)]
struct Link {
    next: Option<NonNull<Link>>,
    prev: Option<NonNull<Link>>,
}

// NodeLink specifically typed for BinderNode
type NodeLink = Link;

// DeathLink specifically typed for DeathEntry
type DeathLink = Link;

// DeathEntry with intrusive link
pub struct DeathEntry {
    link: UnsafeCell<DeathLink>,
    cookie: u64,
    recipient: Arc<dyn DeathRecipient>,
}

unsafe impl Send for DeathEntry {}
unsafe impl Sync for DeathEntry {}

impl DeathEntry {
    fn new(recipient: Arc<dyn DeathRecipient>, cookie: u64) -> Self {
        Self {
            link: UnsafeCell::new(Link::default()),
            cookie,
            recipient,
        }
    }

    // Unsafe accessors (used only under locks)
    unsafe fn link_mut(&mut self) -> &mut DeathLink {
        &mut *self.link.get()
    }

    unsafe fn link(&self) -> &DeathLink {
        &*self.link.get()
    }
}

// Handle for DeathEntry (RAII release/unlink)
pub struct DeathHandle {
    ptr: NonNull<DeathEntry>,
    node_ptr: NonNull<BinderNode>,
}

impl Drop for DeathHandle {
    fn drop(&mut self) {
        self.unlink();
    }
}

impl DeathHandle {
    pub fn unlink(&mut self) {
        let node = unsafe { self.node_ptr.as_ref() };
        let _guard = node.death_lock.lock().unwrap();
        let entry = unsafe { self.ptr.as_mut() };
        Self::remove_from_list(&mut node.death_head.get(), entry);
    }

    unsafe fn remove_from_list(head_ptr: *mut Option<NonNull<DeathEntry>>, entry: &mut DeathEntry) {
        let link = entry.link_mut();
        if let Some(next) = link.next {
            (*next.as_ptr() as *mut DeathEntry).link_mut().prev = link.prev;
        }
        if let Some(prev) = link.prev {
            (*prev.as_ptr() as *mut DeathEntry).link_mut().next = link.next;
        } else {
            // Was head
            *head_ptr = link.next;
        }
        link.next = None;
        link.prev = None;
    }
}

// Main BinderNode structure
struct BinderNode {
    // Intrusive link for manager's live/dead lists
    node_link: UnsafeCell<NodeLink>,
    // Death notifications intrusive list head
    death_head: UnsafeCell<Option<NonNull<DeathEntry>>>,
    // Lock protecting death list mutations and traversal
    death_lock: Mutex<()>,
    // Reference counts
    strong_refs: AtomicUsize,
    weak_refs: AtomicUsize,
    // Backlink to manager
    manager: Arc<NodeManager>,
    // Represented object pointer (simplified)
    object_ptr: *mut (),
}

unsafe impl Send for BinderNode {}
unsafe impl Sync for BinderNode {}

impl BinderNode {
    fn new(manager: Arc<NodeManager>, object_ptr: *mut ()) -> Self {
        Self {
            node_link: UnsafeCell::new(NodeLink::default()),
            death_head: UnsafeCell::new(None),
            death_lock: Mutex::new(()),
            strong_refs: AtomicUsize::new(1), // Initial strong ref
            weak_refs: AtomicUsize::new(0),
            manager,
            object_ptr,
        }
    }

    unsafe fn node_link_mut(&mut self) -> &mut NodeLink {
        &mut *self.node_link.get()
    }

    unsafe fn node_link(&self) -> &NodeLink {
        &*self.node_link.get()
    }

    unsafe fn death_head_mut(&mut self) -> &mut Option<NonNull<DeathEntry>> {
        &mut *self.death_head.get()
    }
}

// RAII handle for BinderNode
#[derive(Clone)]
pub struct NodeHandle {
    ptr: NonNull<BinderNode>,
}

impl NodeHandle {
    pub fn object_ptr(&self) -> *mut () {
        unsafe { self.ptr.as_ref().object_ptr }
    }

    pub fn acquire_strong(&self) {
        let node = unsafe { self.ptr.as_ref() };
        let prev = node.strong_refs.fetch_add(1, Ordering::Acquire);
        if prev == 0 {
            // Node was dead, invalid usage (panic or log in real impl)
            panic!("Acquire on dead node");
        }
    }

    pub fn release_strong(&self) {
        let node = unsafe { self.ptr.as_ref() };
        let prev = node.strong_refs.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            // Last strong ref
            node.manager.queue_for_cleanup(self.ptr);
        }
    }
}

impl Drop for NodeHandle {
    fn drop(&mut self) {
        self.release_strong();
    }
}

// Manager inner state
struct NodeManagerInner {
    live_head: Option<NonNull<BinderNode>>,
    dead_head: Option<NonNull<BinderNode>>,
}

// Main manager
#[derive(Clone)]
pub struct NodeManager {
    inner: Mutex<NodeManagerInner>,
}

impl NodeManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(NodeManagerInner {
                live_head: None,
                dead_head: None,
            }),
        })
    }

    pub fn create_node(&self, object_ptr: *mut ()) -> NodeHandle {
        let node_box = Box::new(BinderNode::new(self.clone(), object_ptr));
        let node_ptr = NonNull::new(Box::into_raw(node_box)).unwrap();
        let mut inner = self.inner.lock().unwrap();
        Self::insert_node_head(&mut inner.live_head, node_ptr);
        NodeHandle { ptr: node_ptr }
    }

    pub fn register_death_recipient(
        &self,
        node_handle: &NodeHandle,
        recipient: Arc<dyn DeathRecipient>,
        cookie: u64,
    ) -> DeathHandle {
        let node = unsafe { node_handle.ptr.as_ref() };
        let _guard = node.death_lock.lock().unwrap();
        let mut entry_box = Box::new(DeathEntry::new(recipient, cookie));
        let entry_ptr = NonNull::new(Box::into_raw(entry_box.as_mut())).unwrap();
        unsafe {
            Self::insert_death_head(node.death_head_mut(), entry_ptr.as_mut());
        }
        DeathHandle {
            ptr: entry_ptr,
            node_ptr: node_handle.ptr,
        }
    }

    fn insert_node_head(head: &mut Option<NonNull<BinderNode>>, node_ptr: NonNull<BinderNode>) {
        unsafe {
            let node = node_ptr.as_mut();
            let link = node.node_link_mut();
            link.prev = None;
            link.next = (*head).map(|h| {
                // Update old head's prev
                let old_node = h.as_mut();
                old_node.node_link_mut().prev = Some(node_ptr);
                h
            });
            *head = Some(node_ptr);
        }
    }

    fn insert_death_head(head: &mut Option<NonNull<DeathEntry>>, entry_ptr: &mut DeathEntry) {
        unsafe {
            let link = entry_ptr.link_mut();
            link.prev = None;
            link.next = *head;
            if let Some(old_head) = *head {
                let old_entry = old_head.as_mut();
                old_entry.link_mut().prev = Some(entry_ptr.into());
            }
            *head = Some(entry_ptr.into());
        }
    }

    fn queue_for_cleanup(&self, node_ptr: NonNull<BinderNode>) {
        let mut inner = self.inner.lock().unwrap();
        unsafe {
            let node = node_ptr.as_mut();
            let link = node.node_link_mut();

            // Unlink from live
            if let Some(next_ptr) = link.next {
                let next_node = next_ptr.as_mut();
                next_node.node_link_mut().prev = link.prev;
            }
            if let Some(prev_ptr) = link.prev {
                let prev_node = prev_ptr.as_mut();
                prev_node.node_link_mut().next = link.next;
            } else if inner.live_head == Some(node_ptr) {
                inner.live_head = link.next;
            }

            // Clear link
            link.next = None;
            link.prev = None;

            // Append to dead (or insert head for simplicity)
            Self::insert_node_head(&mut inner.dead_head, node_ptr);
        }
    }

    /// Process all pending dead nodes (call periodically or as needed)
    /// Handles high-concurrency by releasing lock between each cleanup
    pub fn process_dead(&self) {
        loop {
            let dead_ptr_opt;
            {
                let mut inner = self.inner.lock().unwrap();
                dead_ptr_opt = inner.dead_head.take();
                if dead_ptr_opt.is_none() {
                    return;
                }
                // Unlink first dead (already head, update)
                if let Some(dead_ptr) = dead_ptr_opt {
                    unsafe {
                        let link = dead_ptr.as_ref().node_link();
                        inner.dead_head = link.next;
                        if let Some(next_dead) = link.next {
                            let next_node = next_dead.as_mut();
                            next_node.node_link_mut().prev = None;
                        }
                    }
                }
            } // drop lock

            if let Some(dead_ptr) = dead_ptr_opt {
                self.cleanup_single_node(dead_ptr);
            }
        }
    }

    fn cleanup_single_node(&self, node_ptr: NonNull<BinderNode>) {
        let node = unsafe { node_ptr.as_ref() };
        // Drain death notifications
        let mut death_ptrs = Vec::new();
        let mut cur_head = unsafe { *node.death_head.get() };
        {
            let _guard = node.death_lock.lock().unwrap();
            let mut cur = cur_head;
            while let Some(c) = cur {
                death_ptrs.push(c);
                unsafe {
                    cur = (*c.as_ptr() as *const DeathEntry).link().next;
                }
            }
            unsafe { *node.death_head.get() = None };
        } // drop death lock

        // Notify outside locks
        for death_ptr in death_ptrs {
            unsafe {
                let entry = &*death_ptr.as_ptr();
                entry.recipient.notify(entry.cookie);
                drop(Box::from_raw(death_ptr.as_ptr() as *mut DeathEntry));
            }
        }

        // Free node
        unsafe { drop(Box::from_raw(node_ptr.as_ptr() as *mut BinderNode)); }
    }
}
