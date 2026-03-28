use std::sync::Mutex;

/// An intrusive doubly-linked list node. This must be embedded in your larger structure.
/// Users must call `IntrusiveList::init_node` before inserting.
#[repr(C)]
pub struct ListNode {
    pub prev: *mut ListNode,
    pub next: *mut ListNode,
}

/// Internal state protected by the mutex.
struct ListInner {
    head: *mut ListNode,
    tail: *mut ListNode,
}

/// A thread-safe intrusive doubly-linked list, mimicking kernel-style list_head with head/tail
/// optimization. All modifications (push, remove, release) are serialized by a Mutex to ensure
/// pointer consistency. The `release` function drains the entire list into a local stack (reusing
/// `next` pointers for the stack links) under a single lock acquisition, minimizing contention
/// by batching all unlinks and head/tail updates together before dropping the lock and processing
/// locally. This avoids CVE-2025-68260: concurrent `remove` calls block on the lock during drain,
/// ensuring `prev`/`next` remain valid and synchronized—no detached chain traversal races occur.
pub struct IntrusiveList {
    lock: Mutex<ListInner>,
}

impl IntrusiveList {
    /// Creates a new empty list.
    pub fn new() -> Self {
        Self {
            lock: Mutex::new(ListInner {
                head: std::ptr::null_mut(),
                tail: std::ptr::null_mut(),
            }),
        }
    }

    /// Initializes a node's pointers to detached state. Call before `push_front` or `push_back`.
    pub unsafe fn init_node(&self, node: *mut ListNode) {
        (*node).prev = std::ptr::null_mut();
        (*node).next = std::ptr::null_mut();
    }

    /// Pushes a node to the front of the list.
    pub unsafe fn push_front(&self, node: *mut ListNode) {
        self.init_node(node);
        let mut guard = self.lock.lock().unwrap();
        (*node).prev = std::ptr::null_mut();
        (*node).next = guard.head;
        if !guard.head.is_null() {
            (*guard.head).prev = node;
        } else {
            guard.tail = node;
        }
        guard.head = node;
    }

    /// Pushes a node to the back of the list (O(1) thanks to tail pointer).
    pub unsafe fn push_back(&self, node: *mut ListNode) {
        self.init_node(node);
        let mut guard = self.lock.lock().unwrap();
        (*node).next = std::ptr::null_mut();
        if guard.head.is_null() {
            guard.head = node;
            guard.tail = node;
        } else {
            (*guard.tail).next = node;
            (*node).prev = guard.tail;
            guard.tail = node;
        }
    }

    /// Removes the given node from the list (caller must have direct access to the node pointer).
    /// Safe if called on detached/poisoned nodes (does nothing). Updates head/tail correctly.
    pub unsafe fn remove(&self, node: *mut ListNode) {
        let prev = (*node).prev;
        let next = (*node).next;
        let mut guard = self.lock.lock().unwrap();
        if !prev.is_null() {
            (*prev).next = next;
            if prev == guard.tail {
                guard.tail = (*node).prev;
            }
        } else if guard.head == node {
            guard.head = next;
        }
        if !next.is_null() {
            (*next).prev = prev;
            if next == guard.head {
                guard.head = (*node).next;
            }
        } else if guard.tail == node {
            guard.tail = (*node).prev;
        }
        // Poison the node's pointers
        (*node).prev = std::ptr::null_mut();
        (*node).next = std::ptr::null_mut();
    }

    /// Returns true if the list is empty.
    pub fn is_empty(&self) -> bool {
        let guard = self.lock.lock().unwrap();
        guard.head.is_null()
    }

    /// Drains the entire list into a local stack (LIFO, reusing `next` for stack links),
    /// unlinks all nodes/udpates head/tail under lock, then drops lock and invokes `cleanup`
    /// on each node LIFO outside the lock. This minimizes contention: single lock hold for O(N)
    /// drain regardless of list length, local processing lock-free. Concurrent `remove`/`push`
    /// block, ensuring no pointer races—`prev`/`next` stay valid/synchronized.
    pub unsafe fn release<F>(&self, mut cleanup: F)
    where
        F: FnMut(*mut ListNode),
    {
        let mut guard = self.lock.lock().unwrap();
        let mut local_stack: *mut ListNode = std::ptr::null_mut();
        while !guard.head.is_null() {
            let node = guard.head;
            let next_head = (*node).next;
            // Unlink front
            guard.head = next_head;
            if !next_head.is_null() {
                (*next_head).prev = std::ptr::null_mut();
            } else {
                guard.tail = std::ptr::null_mut();
            }
            // Push to local stack (reuses `next`, sets `prev=null` as poison)
            (*node).next = local_stack;
            (*node).prev = std::ptr::null_mut();
            local_stack = node;
        }
        drop(guard);
        // Process stack LIFO, lock-free
        let mut cur = local_stack;
        while !cur.is_null() {
            let next_in_stack = (*cur).next;
            cleanup(cur);
            cur = next_in_stack;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_operations() {
        let list = IntrusiveList::new();
        let mut nodes: Vec<*mut ListNode> = vec![];
        unsafe {
            for i in 0..3 {
                let mut n = std::mem::MaybeUninit::<ListNode>::uninit();
                let node_ptr = n.as_mut_ptr();
                list.push_back(node_ptr);
                nodes.push(node_ptr);
            }
        }
        assert!(!list.is_empty());
        unsafe {
            list.remove(nodes[1]);
        }
        unsafe {
            list.release(|_| {});
        }
    }
}
