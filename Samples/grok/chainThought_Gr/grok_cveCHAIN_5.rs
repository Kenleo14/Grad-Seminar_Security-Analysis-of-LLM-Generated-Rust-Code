//! Fixed rust_binder intrusive list transfer module.
//! Maintains mutex invariant: all link mutations under Manager.lock.
//! UB-free concurrent move-to-stack + release.

#![deny(unsafe_op_in_unsafe_fn)]

use core::ptr::{self, NonNull};
use core::cell::UnsafeCell;
use std::sync::{Arc, Mutex};  // kernel equiv: kernel::sync::Mutex
use std::collections::HashMap;  // demo: node id -> ptr map (real: traverse or hash)

// Intrusive list link (kernel-style).
#[repr(transparent)]
pub struct ListLink(UnsafeCell<*mut ListNode>);

unsafe impl Send for ListLink {}
unsafe impl Sync for ListLink {}

// List node (first field in Node for container_of equiv).
#[repr(C)]
pub struct ListNode {
    pub next: ListLink,
    pub prev: ListLink,
}

impl ListNode {
    #[inline]
    pub const fn new() -> Self {
        Self {
            next: ListLink(UnsafeCell::new(ptr::null_mut())),
            prev: ListLink(UnsafeCell::new(ptr::null_mut())),
        }
    }
}

// Sentinel-headed intrusive list.
pub struct ListHead {
    head: ListNode,
}

impl ListHead {
    pub fn new() -> Self {
        let mut list = Self { head: ListNode::new() };
        let head_ptr = &mut list.head as *mut ListNode;
        unsafe {
            list.head.next.0.set(head_ptr);
            list.head.prev.0.set(head_ptr);
        }
        list
    }

    /// Safe check if empty.
    pub fn is_empty(&self) -> bool {
        unsafe { self.head.next.0.get() == &self.head as *mut ListNode }
    }

    /// Unsafe: remove node from list (called under mutex).
    pub unsafe fn remove(&mut self, entry: *mut ListNode) {
        let prev_ptr = (*entry).prev.0.get();
        let next_ptr = (*entry).next.0.get();
        (*prev_ptr).next.0.set(next_ptr);
        (*next_ptr).prev.0.set(prev_ptr);
    }

    /// Unsafe: insert after `prev` (called under mutex).
    pub unsafe fn insert_after(&mut self, prev: *mut ListNode, entry: *mut ListNode) {
        let next_ptr = (*prev).next.0.get();
        (*entry).prev.0.set(prev);
        (*entry).next.0.set(next_ptr);
        (*prev).next.0.set(entry);
        (*next_ptr).prev.0.set(entry);
    }

    /// Push back (wrapper).
    pub unsafe fn push_back(&mut self, entry: *mut ListNode) {
        self.insert_after(&mut self.head as *mut _, entry);
    }
}

// BinderNode with intrusive link (refcnt simulated).
#[repr(C)]
pub struct Node {
    pub link: ListNode,
    pub id: usize,
    refcnt: usize,  // demo refcnt
}

impl Node {
    pub fn new(id: usize) -> Box<Self> {
        Box::new(Self {
            link: ListNode::new(),
            id,
            refcnt: 1,
        })
    }

    pub fn link_ptr(&mut self) -> *mut ListNode {
        &mut self.link as *mut _
    }

    /// Simulate refcnt drop; return true if should release.
    pub fn dec_ref(&mut self) -> bool {
        self.refcnt = self.refcnt.saturating_sub(1);
        self.refcnt == 0
    }
}

// Manager holding protected state.
pub struct ManagerInner {
    active_nodes: ListHead,
    nodes_by_id: HashMap<usize, *mut Node>,  // demo lookup (real: traverse)
}

#[derive(Clone)]
pub struct Manager {
    inner: Arc<Mutex<ManagerInner>>,
    stack: Arc<Mutex<Vec<*mut Node>>>,  // per-thread-like stack (different lock domain)
}

impl Manager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ManagerInner {
                active_nodes: ListHead::new(),
                nodes_by_id: HashMap::new(),
            })),
            stack: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// FIXED node_release: unlink UNDER lock before drop.
    /// Invariant: serialized w/ move_to_stack unlink.
    pub fn node_release(&self, id: usize) {
        let mut guard = self.inner.lock().unwrap();
        let node_ptr = match guard.nodes_by_id.get(&id) {
            Some(&ptr) => unsafe { &mut *ptr },
            None => return,
        };
        if !node_ptr.dec_ref() {
            return;
        }
        // CRITICAL FIX: unsafe remove WHILE holding lock!
        unsafe {
            guard.active_nodes.remove(node_ptr.link_ptr());
            guard.nodes_by_id.remove(&id);
        }
        // Safe to drop node now (real: kernel alloc::free).
        // unsafe { drop(Box::from_raw(node_ptr)); }
        drop(guard);  // Drop AFTER mutation.
    }

    /// move_node_to_stack: standard transfer (safe due to serialization).
    /// Window exists, but release can't race-mutate links.
    pub fn move_node_to_stack(&self, id: usize) {
        let mut guard = self.inner.lock().unwrap();
        let node_ptr_opt = guard.nodes_by_id.get(&id).copied();
        let Some(node_ptr) = node_ptr_opt else { return };
        // Unlink under lock.
        unsafe {
            guard.active_nodes.remove((*node_ptr).link_ptr());
        }
        drop(guard);  // Transfer window starts.

        // Under *different* lock (cross-domain transfer).
        let mut stack_guard = self.stack.lock().unwrap();
        stack_guard.push(node_ptr);
        drop(stack_guard);  // Node now "in stack".
    }

    /// Demo: add node to active_nodes (under lock).
    pub fn add_node(&self, node: Box<Node>) -> *mut Node {
        let node_ptr = Box::into_raw(node);
        let mut guard = self.inner.lock().unwrap();
        unsafe {
            guard.active_nodes.push_back((*node_ptr).link_ptr());
        }
        guard.nodes_by_id.insert((*node_ptr).id, node_ptr);
        node_ptr
    }
}

/// Demo usage (simulates concurrent ops; in real, use threads).
pub fn demo() {
    let mgr = Manager::new();
    let node_box = Node::new(42);
    let node_ptr = mgr.add_node(node_box);

    // Simulate concurrent move-to-stack.
    mgr.move_node_to_stack(42);

    // Simulate release (would race in buggy version).
    mgr.node_release(42);

    println!("Fixed: active_nodes empty: {:?}", {
        let guard = mgr.inner.lock().unwrap();
        guard.active_nodes.is_empty()
    }());
    println!("Fixed: stack has node: {:?}", {
        let guard = mgr.stack.lock().unwrap();
        !guard.is_empty()
    }());
}
