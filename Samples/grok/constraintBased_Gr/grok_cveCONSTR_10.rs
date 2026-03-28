use std::ptr;
use std::sync::{Mutex, MutexGuard};

#[repr(C)]
pub struct ListNode {
    pub prev: *mut ListNode,
    pub next: *mut ListNode,
}

struct ListHead {
    head: *mut ListNode,
    tail: *mut ListNode,
}

pub struct IntrusiveList {
    inner: Mutex<ListHead>,
}

impl IntrusiveList {
    /// Creates a new empty intrusive list.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(ListHead {
                head: ptr::null_mut(),
                tail: ptr::null_mut(),
            }),
        }
    }

    /// Initializes a list node with null pointers.
    pub fn init_node(node: *mut ListNode) {
        unsafe {
            (*node).prev = ptr::null_mut();
            (*node).next = ptr::null_mut();
        }
    }

    /// Checks if the list is empty.
    pub fn is_empty(&self) -> bool {
        let guard = self.inner.lock().unwrap();
        guard.head.is_null()
    }

    /// Pushes a node to the front of the list.
    pub fn push_front(&self, node: *mut ListNode) {
        let mut guard = self.inner.lock().unwrap();
        unsafe {
            (*node).prev = ptr::null_mut();
            (*node).next = guard.head;
            if !guard.head.is_null() {
                (*guard.head).prev = node;
            } else {
                guard.tail = node;
            }
            guard.head = node;
        }
    }

    /// Pushes a node to the back of the list.
    pub fn push_back(&self, node: *mut ListNode) {
        let mut guard = self.inner.lock().unwrap();
        unsafe {
            (*node).next = ptr::null_mut();
            (*node).prev = guard.tail;
            if guard.tail.is_null() {
                guard.head = node;
            } else {
                (*guard.tail).next = node;
            }
            guard.tail = node;
        }
    }

    /// Removes a node from the list. Safe concurrently with release(): if list head is null,
    /// skips unlinking to avoid racing the cleanup traversal (fixes CVE-2025-68260 race).
    pub fn remove(&self, node: *mut ListNode) {
        let mut guard = self.inner.lock().unwrap();
        unsafe {
            let p = (*node).prev;
            let n = (*node).next;
            if p.is_null() && n.is_null() {
                // Already removed.
                return;
            }
            // CRITICAL: Skip unlinking if list is drained (head null), to keep prev/next valid
            // for concurrent release() traversal. This avoids the CVE-2025-68260 race condition
            // where remove() would mutate pointers in the old chain during post-drain traversal.
            if guard.head.is_null() {
                return;
            }
            if p.is_null() {
                guard.head = n;
            } else {
                (*p).next = n;
            }
            if n.is_null() {
                guard.tail = p;
            } else {
                (*n).prev = p;
            }
            (*node).prev = ptr::null_mut();
            (*node).next = ptr::null_mut();
        }
    }

    /// Thread-safe cleanup routine. Acquires lock briefly (O(1) time) to drain head/tail,
    /// moves all nodes to a local stack (Vec) via traversal outside lock (minimizes contention).
    /// Concurrent remove() calls after drain see head null and skip unlinking, keeping
    /// prev/next pointers valid/unchanged for traversal.
    pub fn release<F>(&self, mut cleanup: F)
    where
        F: FnMut(*mut ListNode),
    {
        let old_head;
        {
            let mut guard = self.inner.lock().unwrap();
            old_head = guard.head;
            guard.head = ptr::null_mut();
            guard.tail = ptr::null_mut();
            // Explicit drop not needed; scope ends.
        } // Lock released here: O(1) hold time.

        if old_head.is_null() {
            return;
        }

        // Traverse old chain outside lock: safe because concurrent remove() skips on head null.
        let mut local_stack: Vec<*mut ListNode> = Vec::new();
        let mut cur = old_head;
        unsafe {
            while !cur.is_null() {
                local_stack.push(cur);
                cur = (*cur).next;
            }
        }

        // Cleanup without holding lock.
        for node in local_stack {
            cleanup(node);
        }
    }
}
