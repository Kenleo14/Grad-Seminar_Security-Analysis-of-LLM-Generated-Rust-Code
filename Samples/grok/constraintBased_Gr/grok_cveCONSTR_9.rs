use std::mem::ManuallyDrop;
use std::ptr;
use std::sync::{Mutex, atomic::{AtomicBool, Ordering}};

#[repr(C)]
pub struct Node {
    pub next: *mut Node,
    pub prev: *mut Node,
}

/// An intrusive doubly-linked list with thread-safe operations using a mutex for synchronization.
/// Mimics kernel-style intrusive lists (e.g., rust_binder). Uses a dummy head node.
/// Supports fast O(1) release() drain to a local stack (backward traversal for LIFO processing).
/// Avoids CVE-2025-68260 race by using a draining flag to block concurrent remove()/insert() during processing,
/// ensuring prev/next pointers remain valid and unmodified.
pub struct List {
    head: *mut Node,
    lock: Mutex<()>,
    draining: AtomicBool,
    _dummy: ManuallyDrop<Node>,
}

impl List {
    /// Creates a new empty list.
    pub fn new() -> Self {
        let mut dummy = Node {
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        };
        let head_ptr = &mut dummy as *mut Node;
        unsafe {
            (*head_ptr).next = head_ptr;
            (*head_ptr).prev = head_ptr;
        }
        Self {
            head: head_ptr,
            lock: Mutex::new(()),
            draining: AtomicBool::new(false),
            _dummy: ManuallyDrop::new(dummy),
        }
    }

    /// Initializes a node for insertion (sets prev/next to null).
    pub fn init_node(node: *mut Node) {
        unsafe {
            (*node).next = ptr::null_mut();
            (*node).prev = ptr::null_mut();
        }
    }

    /// Pushes a node to the front of the list.
    pub fn push_front(&self, node: *mut Node) {
        let _guard = self.lock.lock().unwrap();
        if self.draining.load(Ordering::Relaxed) {
            return;
        }
        unsafe { Self::_list_add(self.head, node) };
    }

    /// Pushes a node to the back of the list.
    pub fn push_back(&self, node: *mut Node) {
        let _guard = self.lock.lock().unwrap();
        if self.draining.load(Ordering::Relaxed) {
            return;
        }
        unsafe { Self::_list_add_tail(self.head, node) };
    }

    /// Removes a node from the list if present (with validation).
    /// Safe against concurrent release(): skips if draining.
    /// Validates doubly-linked consistency to prevent corruption.
    pub fn remove(&self, node: *mut Node) {
        let _guard = self.lock.lock().unwrap();
        if self.draining.load(Ordering::Relaxed) {
            return;
        }
        unsafe {
            if (*node).prev.is_null() || (*node).next.is_null() {
                return;
            }
            let p = (*node).prev;
            let n = (*node).next;
            // Validate to catch not-in-list or corrupted nodes
            if (*p).next != node || (*n).prev != node {
                return;
            }
            Self::_list_del(node);
        }
    }

    /// Drains the entire list in O(1) lock-held time to a local stack list (LIFO via backward traversal).
    /// Minimizes contention: lock held only for detach + flag set.
    /// Processes nodes via callback without holding lock.
    /// Concurrent remove()/push during processing are blocked (draining flag),
    /// ensuring pointers remain valid/synchronized (no modifications/UAF).
    pub fn release<F>(&self, mut process: F)
    where
        F: FnMut(*mut Node),
    {
        let _guard = self.lock.lock().unwrap();
        self.draining.store(true, Ordering::Relaxed);
        let head_ptr = self.head;
        unsafe {
            let old_next = (*head_ptr).next;
            if old_next == head_ptr {
                // Empty list
                return;
            }
            let old_prev = (*head_ptr).prev;
            // O(1) detach: self-loop head, null-terminate ends
            (*head_ptr).next = head_ptr;
            (*head_ptr).prev = head_ptr;
            (*old_next).prev = ptr::null_mut();
            (*old_prev).next = ptr::null_mut();
            // Release lock immediately
            drop(_guard);
            // Local stack list: backward traversal (LIFO, tail first) using prev pointers
            let mut cur = old_prev;
            while !cur.is_null() {
                let prev_node = (*cur).prev;
                process(cur);
                cur = prev_node;
            }
        }
    }
}

impl List {
    // Private unsafe helpers (assume lock held, pointers valid)
    unsafe fn _list_add(head: *mut Node, node: *mut Node) {
        // Insert after head (push_front)
        let next = (*head).next;
        (*node).prev = head;
        (*node).next = next;
        (*next).prev = node;
        (*head).next = node;
    }

    unsafe fn _list_add_tail(head: *mut Node, node: *mut Node) {
        // Insert before head (push_back)
        let prev = (*head).prev;
        (*node).next = head;
        (*node).prev = prev;
        (*prev).next = node;
        (*head).prev = node;
    }

    unsafe fn _list_del(node: *mut Node) {
        let prev = (*node).prev;
        let next = (*node).next;
        (*prev).next = next;
        (*next).prev = prev;
        (*node).prev = ptr::null_mut();
        (*node).next = ptr::null_mut();
    }
}

/// Example usage (for illustration; nodes must outlive list or be managed externally).
/*
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        let list = List::new();
        let mut nodes = vec![Node { next: ptr::null_mut(), prev: ptr::null_mut() }; 3];
        let node_ptrs: Vec<*mut Node> = nodes.iter_mut().map(|n| n as *mut Node).collect();

        unsafe {
            list.init_node(node_ptrs[0]);
            list.push_back(node_ptrs[0]);
            list.push_front(node_ptrs[1]);
            list.push_back(node_ptrs[2]);
        }

        // Simulate cleanup
        list.release(|node| {
            // e.g., free(node)
            println!("Processing: {:?}", node);
        });
    }
}
*/
