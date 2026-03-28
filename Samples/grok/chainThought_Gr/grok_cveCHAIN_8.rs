//! Simplified model of rust_binder's Node/Alloc intrusive list fix for CVE-2024-36971.
//! Demonstrates fixed Node::release and Alloc::buffer_move_to_stack.
//! Uses std for demo (kernel uses kernel::sync::MutexGuard which derefs mutably).
//! Assumes Node::active_link at offset 0 (#[repr(C)]); real kernel uses offset_of!.
//! Compiles with `rustc --edition=2021 -Zunstable-options` (NonNull<>()), but for illustration.

use std::fmt;
use std::ptr::NonNull;
use std::sync::Mutex;

/// Intrusive doubly-linked list head (kernel style).
#[repr(C)]
#[derive(Debug)]
pub struct ListHead {
    pub next: NonNull<ListHead>,
    pub prev: NonNull<ListHead>,
}

impl ListHead {
    /// Create uninitialized (for stack).
    pub fn new() -> Self {
        unsafe { std::mem::zeroed() }
    }

    /// Initialize as sentinel (empty list).
    pub fn init(&mut self) {
        self.next = NonNull::from(self);
        self.prev = NonNull::from(self);
    }
}

/// Safe to call multiple times (idempotent).
pub unsafe fn list_del_init(link: &mut ListHead) {
    if link.next != NonNull::from(link) || link.prev != NonNull::from(link) {
        // Atomically remove: update neighbors, then init link.
        unsafe {
            (*link.next.as_ptr()).prev = link.prev;
            (*link.prev.as_ptr()).next = link.next;
            link.next = NonNull::from(link);
            link.prev = NonNull::from(link);
        }
    }
}

/// Unsafe container_of: assumes active_link is first field in Node (offset=0).
unsafe fn container_of_link(link_ptr: *mut ListHead) -> *mut Node {
    link_ptr as *mut Node
}

/// Node inner state (protected by node.inner).
#[derive(Debug, Default)]
struct NodeInner {
    strong_refs: usize,
    async_txs: usize,
}

/// Binder Node with intrusive active_link.
#[repr(C)]
#[derive(Debug)]
pub struct Node {
    /// Must be FIRST for container_of(offset=0).
    pub active_link: ListHead,
    pub inner: Mutex<NodeInner>,
    // Other fields (e.g., buffers) omitted.
}

impl Node {
    pub fn new() -> Self {
        Self {
            active_link: ListHead::new(),
            inner: Mutex::new(NodeInner {
                strong_refs: 1,
                ..Default::default()
            }),
        }
    }

    /// FIXED: release() - remove from active_nodes WHILE holding inner.lock.
    /// Drop lock AFTER list mutation.
    pub fn release(&mut self) {
        let mut inner_guard = self.inner.lock().unwrap();
        inner_guard.strong_refs -= 1;
        if inner_guard.strong_refs == 0 {
            // INVARIANT: Mutate active_link under inner.lock.
            // Safe: no concurrent access (move_to_stack blocks on lock).
            unsafe {
                crate::list_del_init(&mut self.active_link);
            }
            // Additional cleanup (e.g., free buffers) here.
        }
        // inner_guard dropped here: list mutation complete.
    }
}

/// Binder Alloc with active_nodes list (protected by alloc.mutex).
#[derive(Debug)]
pub struct BinderAlloc {
    mutex: Mutex<()>,
    /// Sentinel head (mutable access under mutex).
    active_nodes: ListHead,
    // Other fields omitted.
}

impl BinderAlloc {
    pub fn new() -> Self {
        let mut head = ListHead::new();
        head.init();
        Self {
            mutex: Mutex::new(()),
            active_nodes: head,
        }
    }

    /// FIXED: buffer_move_to_stack() - mutate active_link under BOTH alloc.mutex (outer) + node.inner.lock (inner).
    /// Iteration-safe: save next_pos BEFORE locking inner.
    pub fn buffer_move_to_stack(&mut self) {
        let _alloc_guard = self.mutex.lock().unwrap(); // Outer lock: protects list head + iteration.
        let mut pos = self.active_nodes.next;
        while pos != NonNull::from(&mut self.active_nodes) {
            // CRITICAL: save next BEFORE any mutation or locks (safe iteration).
            let next_pos = unsafe { (*pos.as_ptr()).next };

            // Get Node from link.
            let node_ptr = unsafe { container_of_link(pos.as_ptr()) };
            let node = unsafe { &mut *node_ptr };

            // Scope for inner lock.
            {
                let mut inner_guard = node.inner.lock().unwrap(); // Inner lock.
                if inner_guard.async_txs == 0 && inner_guard.strong_refs > 0 {
                    // FIXED: list_del_init WHILE holding inner.lock.
                    // INVARIANT maintained: exclusive access to active_link.
                    unsafe {
                        crate::list_del_init(&mut node.active_link);
                    }
                    // Post-remove: move buffer to stack, etc. (safe under locks).
                    // ...
                }
            } // Drop inner_guard AFTER mutation.

            pos = next_pos; // Advance using saved next (unaffected by del).
        }
    }
}

// Demo helpers (not part of fix).
impl BinderAlloc {
    /// Example: add node to active_nodes (under alloc.mutex).
    pub fn add_active_node(&mut self, node: &mut Node) {
        let _guard = self.mutex.lock().unwrap();
        node.active_link.init();
        unsafe {
            let link_ptr = NonNull::from(&mut node.active_link);
            let first = self.active_nodes.next;
            link_ptr.as_ref().prev = first;
            link_ptr.as_ref().next = NonNull::from(&mut self.active_nodes);
            (*first.as_ptr()).prev = link_ptr;
            self.active_nodes.next = link_ptr;
        }
    }
}

impl fmt::Display for ListHead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ListHead({:?} <-> {:?})", self.prev, self.next)
    }
}
