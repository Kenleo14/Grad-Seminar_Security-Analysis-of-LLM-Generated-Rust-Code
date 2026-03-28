use std::cell::UnsafeCell;
use std::sync::Mutex;

/// An intrusive doubly-linked list node. Users embed this in their own types
/// and manage lifetime externally (e.g., via `Box` or static).
#[repr(C)]
pub struct Node {
    pub next: *mut Node,
    pub prev: *mut Node,
}

unsafe impl Send for Node {}
unsafe impl Sync for Node {}

/// A thread-safe intrusive doubly-linked list protected by a `Mutex<()>`.
/// All operations require holding the lock, ensuring pointer consistency.
/// Mimics patterns in rust_binder with raw pointer manipulation under lock.
pub struct List {
    lock: Mutex<()>,
    head: UnsafeCell<*mut Node>,
}

unsafe impl Send for List {}
unsafe impl Sync for List {}

impl List {
    /// Creates a new empty list.
    pub fn new() -> Self {
        Self {
            lock: Mutex::new(()),
            head: UnsafeCell::new(std::ptr::null_mut()),
        }
    }

    /// Returns `true` if the list is empty.
    pub fn is_empty(&self) -> bool {
        self.with_head(|head| head.is_null())
    }

    /// Atomically executes a closure with read access to the current `head`.
    unsafe fn with_head<F, R>(&self, f: F) -> R
    where
        F: FnOnce(*mut Node) -> R,
    {
        let _guard = self.lock.lock().unwrap();
        let head = *self.head.get();
        f(head)
    }

    /// Atomically executes a closure with mutable access to the `head` pointer.
    /// The closure receives `*mut *mut Node` to allow reading and writing `head`.
    unsafe fn with_head_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(*mut *mut Node) -> R,
    {
        let _guard = self.lock.lock().unwrap();
        let head_ptr = self.head.get();
        f(head_ptr)
    }

    /// Inserts a node at the front of the list (push_front).
    /// Assumes `node` is not already in any list (prev/next null).
    pub fn push_front(&self, node: *mut Node) {
        if node.is_null() {
            return;
        }
        unsafe {
            self.with_head_mut(|head_ptr| {
                let head = *head_ptr;
                (*node).prev = std::ptr::null_mut();
                (*node).next = head;
                if !head.is_null() {
                    (*head).prev = node;
                }
                *head_ptr = node;
            });
        }
    }

    /// Inserts `node` immediately after `prev_node`.
    /// Assumes `prev_node` is in the list, `node` is not in any list.
    pub fn insert_after(&self, prev_node: *mut Node, node: *mut Node) {
        if prev_node.is_null() || node.is_null() {
            return;
        }
        unsafe {
            self.with_head_mut(|head_ptr| {
                let n = (*prev_node).next;
                (*node).next = n;
                (*node).prev = prev_node;
                (*prev_node).next = node;
                if !n.is_null() {
                    (*n).prev = node;
                }
            });
        }
    }

    /// Removes `node` from the list if present.
    /// Validates liveness via bidirectional pointer checks to detect tampering/removal.
    /// Idempotent and safe even if called concurrently with `release()`.
    pub fn remove(&self, node: *mut Node) {
        if node.is_null() {
            return;
        }
        unsafe {
            self.with_head_mut(|head_ptr| {
                let p = (*node).prev;
                let n = (*node).next;

                // Bidirectional validation
                if !p.is_null() && (*p).next != node {
                    return;
                }
                if !n.is_null() && (*n).prev != node {
                    return;
                }

                // Unlink
                if !p.is_null() {
                    (*p).next = n;
                } else if *head_ptr == node {
                    *head_ptr = n;
                } else {
                    // Inconsistent (not head but prev null): ignore
                    return;
                }

                if !n.is_null() {
                    (*n).prev = p;
                }

                // Mark as unlinked
                (*node).prev = std::ptr::null_mut();
                (*node).next = std::ptr::null_mut();
            });
        }
    }

    /// Drains the entire list into a local singly-linked stack (LIFO via `next` pointers)
    /// to minimize lock contention. Unlinks all nodes properly under lock by severing
    /// `prev` pointers sequentially. Returns the stack head pointer.
    /// 
    /// CRITICAL: Avoids CVE-2025-68260 race by:
    /// - Performing full traversal and `prev` updates under lock.
    /// - Repurposing `next` for stack *after* unlinking.
    /// - Post-drain, all nodes have `prev == null`, and `(*next).prev != self`,
    ///   causing concurrent `remove()` validation to fail safely.
    /// Concurrent `remove()` waits for lock, then sees empty list + invalid back-links.
    pub fn release(&self) -> *mut Node {
        unsafe {
            self.with_head_mut(|head_ptr| {
                let mut local_top: *mut Node = std::ptr::null_mut();
                let mut cur = *head_ptr;
                *head_ptr = std::ptr::null_mut();

                while !cur.is_null() {
                    let node = cur;
                    cur = (*node).next;
                    if !cur.is_null() {
                        (*cur).prev = std::ptr::null_mut();
                    }
                    // Repurpose `next` for LIFO stack
                    (*node).next = local_top;
                    local_top = node;
                }
                // All `prev` are now null
                local_top
            })
        }
    }
}

/// Example usage helper: process a stack returned by `release()`.
/// User must provide a closure to cleanup the node (e.g., drop containing struct).
pub unsafe fn process_stack<F>(mut stack_head: *mut Node, mut cleanup: F)
where
    F: FnMut(*mut Node),
{
    while !stack_head.is_null() {
        let node = stack_head;
        stack_head = (*node).next;
        cleanup(node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_ops() {
        unsafe {
            let list = List::new();
            let mut nodes: Vec<*mut Node> = vec![];
            for i in 0..5 {
                let node = Box::into_raw(Box::new(Node {
                    next: std::ptr::null_mut(),
                    prev: std::ptr::null_mut(),
                }));
                nodes.push(node);
                list.push_front(node);
            }

            assert!(!list.is_empty());

            // Remove middle
            list.remove(nodes[2]);

            // Drain
            let stack = list.release();
            assert!(list.is_empty());

            // Process stack (just count)
            let mut count = 0;
            process_stack(stack, |node| {
                count += 1;
                // Leak for test
            });
            assert_eq!(count, 4);
        }
    }

    // Concurrency tests omitted for brevity; use loom or manual threads for verification.
}
