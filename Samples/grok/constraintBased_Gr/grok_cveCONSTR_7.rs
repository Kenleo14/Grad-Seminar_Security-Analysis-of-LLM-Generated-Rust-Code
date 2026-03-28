use std::ptr;
use std::sync::Mutex;

#[repr(C)]
pub struct Link {
    pub prev: *mut Link,
    pub next: *mut Link,
    pub gen: u64,
}

unsafe impl Send for Link {}
unsafe impl Sync for Link {}

pub struct List {
    inner: Mutex<Inner>,
}

struct Inner {
    head: *mut Link,
    tail: *mut Link,
    gen: u64,
}

impl List {
    /// Creates a new empty intrusive linked list.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                head: ptr::null_mut(),
                tail: ptr::null_mut(),
                gen: 0,
            }),
        }
    }
}

    /// Pushes a node to the front of the list.
    /// The caller must ensure the node's `prev`, `next` are null and it is not already linked.
    pub fn push_front(&self, node: *mut Link) {
        if node.is_null() {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        unsafe {
            // Link into list
            let old_head = inner.head;
            (*node).next = old_head;
            (*node).prev = ptr::null_mut();
            if !old_head.is_null() {
                (*old_head).prev = node;
            } else {
                inner.tail = node;
            }
            inner.head = node;
            // Set generation
            (*node).gen = inner.gen;
        }
    }

    /// Pushes a node to the back of the list.
    /// The caller must ensure the node's `prev`, `next` are null and it is not already linked.
    pub fn push_back(&self, node: *mut Link) {
        if node.is_null() {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        unsafe {
            let old_tail = inner.tail;
            (*node).prev = old_tail;
            (*node).next = ptr::null_mut();
            if !old_tail.is_null() {
                (*old_tail).next = node;
            } else {
                inner.head = node;
            }
            inner.tail = node;
            (*node).gen = inner.gen;
        }
    }

    /// Removes a node from the list in O(1) time.
    /// Safe against concurrent `release()` due to generation check.
    /// Idempotent if already removed or stale.
    pub fn remove(&self, node: *mut Link) {
        if node.is_null() {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        unsafe {
            if (*node).gen != inner.gen {
                // Stale node from previous release batch
                return;
            }
            let p = (*node).prev;
            let n = (*node).next;
            if !p.is_null() {
                (*p).next = n;
            } else if inner.head == node {
                inner.head = n;
            }
            if !n.is_null() {
                (*n).prev = p;
            } else if inner.tail == node {
                inner.tail = p;
            }
            (*node).prev = ptr::null_mut();
            (*node).next = ptr::null_mut();
        }
    }

    /// Releases (drains) the entire list into a local stack list (reversed, using `next` pointers).
    /// Minimizes lock hold time to O(1): quick swap of head/tail/gen.
    /// Returns the top of the stack list (`*mut Link`), or null if empty.
    /// The stack is singly-linked via `next` (prev set to null).
    /// Concurrent `remove()` calls on nodes in the stack will skip due to gen mismatch,
    /// ensuring prev/next pointers remain valid and synchronized (no races).
    /// Process the stack outside any lock:
    /// 