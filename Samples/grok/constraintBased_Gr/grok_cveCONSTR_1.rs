use std::ptr;
use std::sync::Mutex;

#[repr(C)]
#[derive(Debug)]
pub struct ListNode {
    pub next: *mut ListNode,
    pub prev: *mut ListNode,
}

#[repr(C)]
pub struct ListHead {
    inner: Mutex<ListNode>,
}

const POISON: *mut ListNode = 1usize as *mut ListNode;

impl ListHead {
    /// Creates a new empty intrusive linked list head (sentinel).
    pub fn new() -> Self {
        let mut node = ListNode {
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        };
        let head_ptr = unsafe { &mut node as *mut ListNode };
        unsafe {
            node.next = head_ptr;
            node.prev = head_ptr;
        }
        Self {
            inner: Mutex::new(node),
        }
    }

    /// Returns true if the list is empty.
    pub fn is_empty(&self) -> bool {
        let guard = self.inner.lock().unwrap();
        let head_ptr = &mut *guard as *mut ListNode;
        unsafe { (*head_ptr).next == head_ptr }
    }

    /// Pushes a node to the back of the list.
    /// The node's `next` and `prev` should be initialized to null or poison before calling.
    pub fn push_back(&self, node: *mut ListNode) {
        if node.is_null() {
            return;
        }
        let mut guard = self.inner.lock().unwrap();
        let head_ptr = &mut *guard as *mut ListNode;
        unsafe {
            let last = (*head_ptr).prev;
            (*node).prev = last;
            (*node).next = head_ptr;
            (*last).next = node;
            (*head_ptr).prev = node;
        }
    }

    /// Pushes a node to the front of the list.
    pub fn push_front(&self, node: *mut ListNode) {
        if node.is_null() {
            return;
        }
        let mut guard = self.inner.lock().unwrap();
        let head_ptr = &mut *guard as *mut ListNode;
        unsafe {
            let first = (*head_ptr).next;
            (*node).next = first;
            (*node).prev = head_ptr;
            (*first).prev = node;
            (*head_ptr).next = node;
        }
    }

    /// Pops and returns the front node, or None if empty.
    pub fn pop_front(&self) -> Option<*mut ListNode> {
        let mut guard = self.inner.lock().unwrap();
        let head_ptr = &mut *guard as *mut ListNode;
        let first = unsafe { (*head_ptr).next };
        if first == head_ptr {
            return None;
        }
        let next_first = unsafe { (*first).next };
        unsafe {
            (*next_first).prev = head_ptr;
            (*head_ptr).next = next_first;
        }
        Some(first)
    }

    /// Removes a specific node from the list if it is present and pointers are synchronized.
    /// Returns true if removed.
    pub fn remove(&self, node: *mut ListNode) -> bool {
        if node.is_null() {
            return false;
        }
        let mut guard = self.inner.lock().unwrap();
        let head_ptr = &mut *guard as *mut ListNode;
        unsafe {
            if (*node).next == POISON || (*node).prev == POISON {
                return false;
            }
            let p = (*node).prev;
            let n = (*node).next;
            if p.is_null() || n.is_null() || (*p).next != node || (*n).prev != node || node == head_ptr {
                return false;
            }
            // Splice out
            (*p).next = n;
            (*n).prev = p;
            // Poison to prevent double-remove
            (*node).next = POISON;
            (*node).prev = POISON;
            true
        }
    }

    /// Thread-safe cleanup routine: drains the entire list by repeatedly popping front nodes
    /// under short critical sections (O(1) lock hold time per node) into a local singly-linked
    /// stack (misusing `prev` field), then invokes the cleanup callback on each without holding
    /// the lock. This minimizes lock contention.
    /// Concurrent `remove()` calls are handled safely via pointer consistency checks.
    pub fn release<F>(&self, mut cleanup: F)
    where
        F: FnMut(*mut ListNode),
    {
        let mut local_top: *mut ListNode = ptr::null_mut();
        loop {
            // Short lock acquisition for O(1) pop_front
            let mut guard = self.inner.lock().unwrap();
            let head_ptr = &mut *guard as *mut ListNode;
            let first = unsafe { (*head_ptr).next };
            if first == head_ptr {
                // Empty, done
                break;
            }
            // Atomic splice out (pointers now inconsistent for this node)
            let next_first = unsafe { (*first).next };
            unsafe {
                (*next_first).prev = head_ptr;
                (*head_ptr).next = next_first;
            }
            drop(guard); // Release lock immediately

            // Push to local stack (singly-linked via `prev`)
            unsafe {
                (*first).prev = local_top;
            }
            local_top = first;
        }
        // Process local stack without lock
        let mut current = local_top;
        while !current.is_null() {
            let node = current;
            current = unsafe { (*node).prev };
            cleanup(node);
        }
    }
}
