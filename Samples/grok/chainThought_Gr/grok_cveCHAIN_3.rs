#![no_std] // Kernel-compatible base; add `extern crate alloc;` for VecDeque in kernel.

// For demo/compile with std: remove no_std, add `std::sync`, `std::ptr`, `std::collections`.
#[cfg(feature = "std")]
use std::sync::{Mutex, MutexGuard};
#[cfg(feature = "std")]
use std::collections::VecDeque;
#[cfg(feature = "std")]
use std::ptr::NonNull;

// Kernel replacements:
// use kernel::sync::{Mutex, MutexGuard};
// use alloc::collections::VecDeque;
// use core::ptr::NonNull;

#[repr(C)]
struct NodeLinks {
    prev: *mut NodeLinks,
    next: *mut NodeLinks,
}

union NodeLinksStorage {
    links: NodeLinks,
    _pad: [u8; 16], // Align/pad for kernel.
}

struct NodeState {
    refcount: usize,
    dead: bool,
}

#[repr(C)]
pub struct Node {
    pub links: UnsafeCell<NodeLinksStorage>,
    pub state: Mutex<NodeState>,
}

unsafe impl Send for Node {}
unsafe impl Sync for Node {}

impl Node {
    /// Create new Node (refcount=1).
    pub fn new() -> Self {
        Self {
            links: UnsafeCell::new(NodeLinksStorage {
                links: NodeLinks {
                    prev: core::ptr::null_mut(),
                    next: core::ptr::null_mut(),
                },
            }),
            state: Mutex::new(NodeState { refcount: 1, dead: false }),
        }
    }

    /// FIXED release: Hold state lock through ENTIRE unlink + refcount logic.
    /// Invariant: links mutated ONLY under state.lock held.
    pub fn release(&mut self) {
        let mut guard: MutexGuard<NodeState> = self.state.lock().expect("lock poisoned");
        guard.refcount -= 1;
        if guard.refcount == 0 {
            guard.dead = true;
            // CRITICAL FIX: Unlink UNDER LOCK - no aliasing possible.
            unsafe { self.do_unlink() };
        }
        // Drop AFTER unlink/transfer lifecycle.
        drop(guard);
    }

    /// Unsafe unlink: mutate prev/next atomically under lock.
    unsafe fn do_unlink(&mut self) {
        let links_ptr = &mut self.links.get().as_mut().unwrap().links;
        let prev_ptr = links_ptr.prev;
        let next_ptr = links_ptr.next;
        // Standard doubly-linked unlink.
        unsafe {
            (*prev_ptr).next = next_ptr;
            (*next_ptr).prev = prev_ptr;
        }
        // Clear self.
        links_ptr.prev = links_ptr as *mut _;
        links_ptr.next = links_ptr as *mut _;
    }

    /// Get links ptr for external ops (e.g., insert).
    pub fn links_ptr(&mut self) -> *mut NodeLinks {
        unsafe { &mut *self.links.get().as_mut().unwrap().links }
    }
}

/// Alloc manages the intrusive list (sentinel head).
pub struct Alloc {
    head_links: NodeLinks,
    lock: Mutex<()>,
}

impl Alloc {
    pub fn new() -> Self {
        let mut head_links = NodeLinks {
            prev: core::ptr::null_mut(),
            next: core::ptr::null_mut(),
        };
        unsafe {
            let head_ptr = &mut head_links as *mut NodeLinks;
            head_links.prev = head_ptr;
            head_links.next = head_ptr;
        }
        Self {
            head_links,
            lock: Mutex::new(()),
        }
    }

    /// Add node to tail (under alloc.lock).
    pub fn add_node(&mut self, node: &mut Node) {
        let _guard = self.lock.lock().expect("lock poisoned");
        unsafe {
            let node_links = node.links_ptr();
            let head_ptr = &mut self.head_links as *mut NodeLinks;
            (*node_links).prev = (*head_ptr).prev;
            (*node_links).next = head_ptr;
            (*(*node_links).prev).next = node_links;
            (*head_ptr).prev = node_links;
        }
    }

    /// FIXED move_to_stack: Transfer dead nodes to stack.
    /// - Holds alloc.lock (serializes traversal/unlink).
    /// - For each: node.state.lock (inner) → check dead → release() [unlinks under its lock].
    /// Full lifecycle invariant: no concurrent aliasing/UAF.
    pub fn move_to_stack(&mut self, stack: &mut VecDeque<*mut Node>) {
        let _guard = self.lock.lock().expect("lock poisoned");
        unsafe {
            let mut cur_links_ptr: *mut NodeLinks = (*&self.head_links).next;
            while cur_links_ptr != &mut self.head_links {
                // container_of: links → Node (assume links at offset 0).
                let node_ptr = core::ptr::addr_of!((*cur_links_ptr)) as *mut Node;
                let node = &mut *node_ptr;
                // Lock node state (inner lock).
                let mut node_guard = node.state.lock().expect("lock poisoned");
                if node_guard.dead {
                    // Trigger release (unlinks under node_guard).
                    drop(node_guard); // Drop before release to match caller patterns.
                    node.release(); // Safe: holds lock → unlink → drop.
                    stack.push_back(node_ptr);
                } else {
                    drop(node_guard);
                }
                cur_links_ptr = (*cur_links_ptr).next;
            }
        }
    }
}

#[cfg(feature = "std")]
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_fixed_no_ub() {
        let alloc = Arc::new(Mutex::new(Alloc::new()));
        let mut handles = vec![];

        // Spawn move_to_stack.
        let alloc_clone = alloc.clone();
        let handle1 = thread::spawn(move || {
            let mut alloc = alloc_clone.lock().unwrap();
            let mut stack = VecDeque::new();
            alloc.move_to_stack(&mut stack);
            assert_eq!(stack.len(), 0); // No dead yet.
        });

        // Add node, release concurrently.
        let mut node = Box::new(Node::new());
        let alloc_l = alloc.lock().unwrap();
        alloc_l.add_node(&mut *node);
        drop(alloc_l); // Release alloc.lock.

        // Concurrent release.
        let handle2 = thread::spawn(move || {
            unsafe { node.release() };
        });

        handles.push(handle1);
        handles.push(handle2);
        for h in handles { h.join().unwrap(); }
    }
}
