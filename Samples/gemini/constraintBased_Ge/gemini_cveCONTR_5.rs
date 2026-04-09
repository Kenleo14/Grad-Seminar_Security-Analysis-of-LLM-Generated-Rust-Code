use std::sync::Mutex;
use std::ptr;

/// A node for an intrusive doubly-linked list.
pub struct IntrusiveNode {
    next: *mut IntrusiveNode,
    prev: *mut IntrusiveNode,
}

impl IntrusiveNode {
    pub fn new() -> Self {
        Self {
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        }
    }

    pub fn is_linked(&self) -> bool {
        !self.next.is_null()
    }
}

/// The intrusive list container.
pub struct IntrusiveList {
    head: *mut IntrusiveNode,
}

impl IntrusiveList {
    pub const fn new() -> Self {
        Self { head: ptr::null_mut() }
    }
}

pub struct BinderManager {
    // The shared list protected by a Mutex.
    list: Mutex<IntrusiveList>,
}

impl BinderManager {
    /// CVE-2025-68260 Mitigation: 
    /// We must ensure that the release routine doesn't assume a node is valid
    /// if it was concurrently removed by another thread.
    pub fn release(&self) {
        let mut local_stack: *mut IntrusiveNode = ptr::null_mut();

        // --- Critical Section Start ---
        {
            let mut guard = self.list.lock().unwrap();
            let mut current = guard.head;

            // Move the entire list to our local stack and clear the shared head.
            // This minimizes lock contention.
            if !current.is_null() {
                local_stack = current;
                guard.head = ptr::null_mut();
            }
        }
        // --- Critical Section End ---

        // Now process the local stack without holding the lock.
        let mut current = local_stack;
        while !current.is_null() {
            unsafe {
                let node = &mut *current;
                let next_node = node.next;

                // Perform actual cleanup logic here
                self.cleanup_node(current);

                // Reset pointers to prevent dangling references
                node.next = ptr::null_mut();
                node.prev = ptr::null_mut();

                current = next_node;
            }
        }
    }

    /// Thread-safe removal of a single node.
    pub fn remove_node(&self, node_ptr: *mut IntrusiveNode) {
        let mut guard = self.list.lock().unwrap();
        unsafe {
            let node = &mut *node_ptr;

            // Check if node is still in a list to prevent double-unlink 
            // or unlinking from the local_stack list.
            if !node.is_linked() {
                return;
            }

            if !node.prev.is_null() {
                (*node.prev).next = node.next;
            } else {
                guard.head = node.next;
            }

            if !node.next.is_null() {
                (*node.next).prev = node.prev;
            }

            node.next = ptr::null_mut();
            node.prev = ptr::null_mut();
        }
    }

    unsafe fn cleanup_node(&self, _node: *mut IntrusiveNode) {
        // Implementation-specific resource deallocation
    }
}