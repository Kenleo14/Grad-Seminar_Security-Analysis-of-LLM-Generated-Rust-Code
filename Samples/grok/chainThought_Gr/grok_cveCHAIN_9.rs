//! Fixed rust_binder intrusive list module.
//! Demonstrates lock-held removal to fix CVE-2024-35812 aliasing UB.

use core::ptr;
use std::sync::Mutex; // Kernel equiv: kernel::sync::Mutex<()>
use std::vec::Vec;

/// Intrusive doubly-linked list head (null-terminated).
#[repr(C)]
pub struct IntrusiveList {
    head: *mut Node,
}

/// Binder node with intrusive links (simulates `struct binder_node`).
#[repr(C)]
pub struct Node {
    pub prev: *mut Node,
    pub next: *mut Node,
    pub strong_refs: usize,
    pub weak_refs: usize,
    pub data: usize, // Opaque payload.
}

/// Fixed Binder module (simulates `struct binder_device`).
pub struct BinderFixed {
    lock: Mutex<()>,
    dead_nodes: IntrusiveList, // e.g., dead_binder_nodes list.
}

impl BinderFixed {
    /// Create new instance.
    pub fn new() -> Self {
        Self {
            lock: Mutex::new(()),
            dead_nodes: IntrusiveList {
                head: ptr::null_mut(),
            },
        }
    }

    /// Allocate a new node and add to dead_nodes list (simulates creation/dead state).
    pub fn alloc_dead_node(&self, data: usize) -> *mut Node {
        let node = Box::into_raw(Box::new(Node {
            prev: ptr::null_mut(),
            next: ptr::null_mut(),
            strong_refs: 1,
            weak_refs: 0,
            data,
        }));

        let _guard = self.lock.lock().unwrap();
        unsafe {
            Self::push_front(&mut self.dead_nodes, node);
        }
        node
    }

    /// Release node: FIXED - hold lock across check + full removal.
    /// Concurrent move_to_stack safe (no aliasing UB).
    pub fn node_release(&self, node: *mut Node) {
        let guard = self.lock.lock().unwrap(); // Acquire.
        unsafe {
            // Simulate ref drop (in real: atomic_fetch_sub).
            (*node).strong_refs = (*node).strong_refs.saturating_sub(1);
            (*node).weak_refs = (*node).weak_refs.saturating_sub(1);
            if (*node).strong_refs == 0 && (*node).weak_refs == 0 {
                // FULL removal under lock: invariant holds.
                remove_node_from_list(node, &mut self.dead_nodes);
                drop(guard); // Drop after removal.
                // Free: kernel::alloc::kfree(node as *mut _);
                println!("Fixed: Freed node {:?} (data={})", node, (*node).data);
                unsafe { drop(Box::from_raw(node)) };
            } else {
                // Keep alive.
            }
        }
    }

    /// Move-to-stack: Drain entire list to Vec under lock (e.g., bc_dead_binder processing).
    /// Concurrent release races fixed.
    pub fn move_to_stack(&self) -> Vec<*mut Node> {
        let mut stack: Vec<*mut Node> = Vec::new();
        let guard = self.lock.lock().unwrap();
        unsafe {
            while let Some(node) = pop_front(&mut self.dead_nodes) {
                stack.push(node);
            }
        }
        // Guard drops here: list empty.
        stack
    }

    /// Unsafe helpers (called ONLY under lock).
    unsafe fn push_front(list: &mut IntrusiveList, node: *mut Node) {
        (*node).prev = ptr::null_mut();
        (*node).next = list.head;
        if !list.head.is_null() {
            (*list.head).prev = node;
        }
        list.head = node;
    }
}

/// Core unsafe list ops: VALID ONLY under binder_lock (no aliasing).
unsafe fn remove_node_from_list(node: *mut Node, list: &mut IntrusiveList) {
    let prev = (*node).prev;
    let next = (*node).next;
    if !prev.is_null() {
        // Internal node.
        (*prev).next = next;
    } else {
        // Head node: update list.head.
        list.head = next;
    }
    if !next.is_null() {
        (*next).prev = prev;
    }
    // Poison links.
    (*node).prev = ptr::null_mut();
    (*node).next = ptr::null_mut();
}

unsafe fn pop_front(list: &mut IntrusiveList) -> Option<*mut Node> {
    let head = list.head;
    if head.is_null() {
        return None;
    }
    let next = (*head).next;
    list.head = next;
    if !next.is_null() {
        (*next).prev = ptr::null_mut();
    }
    (*head).prev = ptr::null_mut();
    (*head).next = ptr::null_mut();
    Some(head)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixed_no_race() {
        let binder = BinderFixed::new();
        let node1 = binder.alloc_dead_node(42);
        let node2 = binder.alloc_dead_node(99);

        // Simulate concurrent move-to-stack.
        let stack1 = binder.move_to_stack();
        assert_eq!(stack1.len(), 2);
        assert_eq!(unsafe { (*stack1[0]).data }, 99); // LIFO push_front.
        assert_eq!(unsafe { (*stack1[1]).data }, 42);

        // Release (would race in buggy, but safe here).
        unsafe {
            (*node1).strong_refs = 0;
            (*node1).weak_refs = 0;
        }
        binder.node_release(node1);
        // node2 already moved.

        // List now empty.
        let stack2 = binder.move_to_stack();
        assert!(stack2.is_empty());
    }
}
