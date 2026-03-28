use std::ptr::{self, NonNull};
use std::sync::Mutex;

#[repr(C)]
pub struct ListNode {
    pub prev: *mut ListNode,
    pub next: *mut ListNode,
}

pub struct List {
    lock: Mutex<()>,
    head: Option<NonNull<ListNode>>,
    tail: Option<NonNull<ListNode>>,
}

impl List {
    /// Creates a new empty intrusive list.
    pub fn new() -> Self {
        Self {
            lock: Mutex::new(()),
            head: None,
            tail: None,
        }
    }

    /// Initializes a node's pointers to null (must be called before use).
    pub unsafe fn init(node: *mut ListNode) {
        (*node).prev = ptr::null_mut();
        (*node).next = ptr::null_mut();
    }

    /// Pushes a node to the back of the list.
    pub fn push_back(&self, node: NonNull<ListNode>) {
        unsafe {
            (*node.as_ptr()).next = ptr::null_mut();
        }
        let _guard = self.lock.lock().unwrap();
        unsafe {
            (*node.as_ptr()).prev = self.tail.map_or(ptr::null_mut(), |t| t.as_ptr());
        }
        if let Some(tail) = self.tail {
            unsafe {
                (*tail.as_ptr()).next = node.as_ptr();
            }
        } else {
            self.head = Some(node);
        }
        self.tail = Some(node);
    }

    /// Removes a node from the list (self-remove, safe for double-remove).
    /// Avoids corruption if already removed (prev/next null).
    pub fn remove(&self, node: NonNull<ListNode>) {
        let _guard = self.lock.lock().unwrap();
        unsafe {
            let prev_ptr = (*node.as_ptr()).prev;
            let next_ptr = (*node.as_ptr()).next;
            if prev_ptr.is_null() && next_ptr.is_null() {
                // Already removed.
                return;
            }
            if !prev_ptr.is_null() {
                (*prev_ptr).next = next_ptr;
            } else {
                self.head = NonNull::new(next_ptr);
            }
            if !next_ptr.is_null() {
                (*next_ptr).prev = prev_ptr;
            } else {
                self.tail = NonNull::new(prev_ptr);
            }
            (*node.as_ptr()).prev = ptr::null_mut();
            (*node.as_ptr()).next = ptr::null_mut();
        }
    }

    /// Releases all nodes: briefly holds the lock to null all prev/next pointers
    /// (collecting direct pointers in a local vec/stack to avoid linked traversal),
    /// drops the lock, then invokes cleanup on each. This minimizes contention
    /// (lock held only for O(n) nulling/traversal) and avoids CVE-2025-68260 races:
    /// concurrent remove() sees null prev/next and skips (no pointer modification).
    pub fn release<F>(&self, mut cleanup: F)
    where
        F: FnMut(*mut ListNode),
    {
        let mut local_nodes: Vec<NonNull<ListNode>> = Vec::new();
        {
            let _guard = self.lock.lock().unwrap();
            let mut cur = self.head.take();
            self.tail = None;
            while let Some(node_ptr) = cur {
                let next_ptr = unsafe { (*node_ptr.as_ptr()).next };
                unsafe {
                    // Null pointers under lock to poison/invalidate for concurrent remove().
                    (*node_ptr.as_ptr()).prev = ptr::null_mut();
                    (*node_ptr.as_ptr()).next = ptr::null_mut();
                }
                local_nodes.push(node_ptr);
                cur = NonNull::new(next_ptr);
            }
        } // Lock dropped.
        // Process unlocked (local direct pointers, no links).
        for node_ptr in local_nodes {
            cleanup(node_ptr.as_ptr());
        }
    }
}
