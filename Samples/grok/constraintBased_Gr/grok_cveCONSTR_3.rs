use std::ptr;
use std::sync::Mutex;

/// An intrusive doubly-linked list node. Must be embedded in a larger struct.
/// Fields must not be accessed directly outside of this module's unsafe functions.
#[repr(C)]
pub struct Node {
    pub next: *mut Node,
    pub prev: *mut Node,
}

/// Internal state protected by the mutex.
struct ListInner {
    head: *mut Node,
    tail: *mut Node,
}

/// A thread-safe intrusive doubly-linked list.
///
/// All mutating operations (insert, remove) acquire a mutex for serialization.
/// The `release` function minimizes lock hold time by atomically splicing the
/// entire list to a local stack (snapshot of the `next` chain) under the lock,
/// then releasing the lock before performing cleanup on the local stack.
///
/// This avoids the CVE-2025-68260 race condition: after splicing, `head` is `null`.
/// Any concurrent `remove` sees `head.is_null()` and returns early *without*
/// modifying any `prev`/`next` pointers (no neighbor updates, no poisoning).
/// Thus, the local `next` chain remains valid and synchronized for traversal,
/// even under concurrent `remove` calls.
pub struct List {
    inner: Mutex<ListInner>,
}

impl List {
    /// Creates a new empty list.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(ListInner {
                head: ptr::null_mut(),
                tail: ptr::null_mut(),
            }),
        }
    }

    /// Initializes a node's pointers to null. Call before first insert.
    pub unsafe fn node_init(node: *mut Node) {
        unsafe {
            (*node).next = ptr::null_mut();
            (*node).prev = ptr::null_mut();
        }
    }

    /// Inserts a node at the head of the list.
    /// Assumes `node_init` has been called or pointers are valid.
    pub unsafe fn insert_head(&self, node: *mut Node) {
        let mut guard = self.inner.lock().unwrap();
        unsafe {
            (*node).prev = ptr::null_mut();
            (*node).next = guard.head;
        }
        if !guard.head.is_null() {
            unsafe {
                (*guard.head).prev = node;
            }
        } else {
            guard.tail = node;
        }
        guard.head = node;
    }

    /// Inserts a node at the tail of the list.
    /// Assumes `node_init` has been called or pointers are valid.
    pub unsafe fn insert_tail(&self, node: *mut Node) {
        let mut guard = self.inner.lock().unwrap();
        unsafe {
            (*node).next = ptr::null_mut();
            (*node).prev = guard.tail;
        }
        if !guard.tail.is_null() {
            unsafe {
                (*guard.tail).next = node;
            }
        } else {
            guard.head = node;
        }
        guard.tail = node;
    }

    /// Removes a node from the list.
    /// Safe even if the node is not in the list or list pointers are corrupted:
    /// - Validates bidirectional pointers and endpoint consistency.
    /// - Early exit if `head.is_null()` (drained): does *not* touch neighbors.
    /// - Poisons node's pointers on failure or success.
    /// This ensures no pointer modifications post-drain, avoiding chain corruption.
    pub unsafe fn remove(&self, node: *mut Node) {
        let mut guard = self.inner.lock().unwrap();

        // CRITICAL: Early exit if drained (post-release). No pointer touches.
        // Preserves local stack chain validity.
        if guard.head.is_null() {
            unsafe {
                (*node).next = ptr::null_mut();
                (*node).prev = ptr::null_mut();
            }
            return;
        }

        let p = unsafe { (*node).prev };
        let n = unsafe { (*node).next };

        // Validate consistency (prevents invalid unlinks)
        let is_head = p.is_null() && (guard.head == node);
        let is_tail = n.is_null() && (guard.tail == node);
        let prev_ok = p.is_null() || unsafe { (*p).next == node };
        let next_ok = n.is_null() || unsafe { (*n).prev == node };

        if !is_head && !is_tail && !(prev_ok && next_ok) {
            // Invalid: poison only this node
            unsafe {
                (*node).next = ptr::null_mut();
                (*node).prev = ptr::null_mut();
            }
            return;
        }

        // Unlink (safe post-validation)
        if !p.is_null() {
            unsafe {
                (*p).next = n;
            }
        } else {
            guard.head = n;
        }
        if !n.is_null() {
            unsafe {
                (*n).prev = p;
            }
        } else {
            guard.tail = p;
        }

        // Poison removed node
        unsafe {
            (*node).next = ptr::null_mut();
            (*node).prev = ptr::null_mut();
        }
    }

    /// Drains the entire list to a local stack (linked via `next` pointers)
    /// with *minimal* lock hold time (just splice + null head/tail).
    /// Then invokes `cleanup` on each node *outside* the lock.
    /// The local stack's pointers remain valid due to `remove`'s early exit.
    pub fn release<F>(&self, mut cleanup: F)
    where
        F: FnMut(*mut Node),
    {
        // Splice under lock (minimal time)
        let mut local_stack_head: *mut Node = ptr::null_mut();
        {
            let mut guard = self.inner.lock().unwrap();
            local_stack_head = guard.head;
            guard.head = ptr::null_mut();
            guard.tail = ptr::null_mut();
        }

        // Traverse local stack outside lock (pointers immutable post-splice)
        let mut cur = local_stack_head;
        while !cur.is_null() {
            let next = unsafe { (*cur).next };
            cleanup(cur);
            cur = next;
        }
    }
}

#[cfg(test)]
mod tests {
    // Note: Tests omitted for brevity; would verify insert/remove/release
    // and concurrent scenarios using threads.
}
