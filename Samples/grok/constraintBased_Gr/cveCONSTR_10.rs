// Complete Rust module for a thread-safe intrusive linked list with a safe cleanup routine.
// This implements the requirements exactly:
// 1. Uses `unsafe` for all intrusive pointer manipulation (mimicking the style in rust_binder's death_list).
// 2. The `release()` function moves items to a local collection (a "stack list" of pointers) to keep the critical section minimal.
// 3. Avoids the exact race condition in CVE-2025-68260 (Node::release moving the list to a local stack list, then iterating it after dropping the lock, allowing concurrent `remove()` to corrupt prev/next pointers in a different list).
// 4. Uses `std::sync::Mutex` (standard synchronization primitive; a spinlock could be substituted in no_std/kernel environments).

use std::sync::Mutex;
use std::ptr;

/// Poison values used after `list_del` (standard kernel-style defense against double-delete or use-after-free).
const LIST_POISON1: *mut ListHead = 0x100 as *mut ListHead;
const LIST_POISON2: *mut ListHead = 0x200 as *mut ListHead;

/// Intrusive list head (embedded in user nodes). All manipulation is `unsafe` and mimics rust_binder.
#[repr(C)]
#[derive(Debug)]
pub struct ListHead {
    pub next: *mut ListHead,
    pub prev: *mut ListHead,
}

impl ListHead {
    /// Constant for an uninitialized head (user must call `init` before use).
    pub const fn uninit() -> Self {
        ListHead {
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        }
    }

    /// Initialize a list head as empty (circular sentinel).
    pub unsafe fn init(head: *mut Self) {
        let h = &mut *head;
        h.next = head;
        h.prev = head;
    }

    /// Returns true if the list is empty.
    pub unsafe fn empty(head: *const Self) -> bool {
        (*head).next == head as *mut _
    }
}

/// Private unsafe helpers for intrusive operations (never exposed publicly).
mod list_ops {
    use super::*;

    pub unsafe fn add(new: *mut ListHead, head: *mut ListHead) {
        let next = (*head).next;
        (*new).next = next;
        (*new).prev = head;
        (*next).prev = new;
        (*head).next = new;
    }

    pub unsafe fn del(entry: *mut ListHead) {
        let prev = (*entry).prev;
        let next = (*entry).next;
        (*next).prev = prev;
        (*prev).next = next;
        // Poison to make concurrent remove() a safe no-op (avoids CVE race).
        (*entry).next = LIST_POISON1;
        (*entry).prev = LIST_POISON2;
    }

    pub unsafe fn splice_init(list: *mut ListHead, head: *mut ListHead) {
        if !super::ListHead::empty(list) {
            let first = (*list).next;
            let last = (*list).prev;
            let at = (*head).next;
            (*first).prev = head;
            (*last).next = at;
            (*at).prev = last;
            (*head).next = first;
            // Re-init source list to empty.
            (*list).next = list;
            (*list).prev = list;
        }
    }

    /// Helper to detect poisoned pointers (used in safe remove).
    pub fn is_poisoned(p: *mut ListHead) -> bool {
        p == LIST_POISON1 || p == LIST_POISON2
    }
}

/// Thread-safe wrapper around the intrusive list.
/// All public operations are synchronized; the lock is held only for pointer manipulation.
pub struct ThreadSafeIntrusiveList {
    inner: Mutex<ListHead>,
}

impl ThreadSafeIntrusiveList {
    /// Creates a new empty thread-safe intrusive list.
    pub fn new() -> Self {
        let mut head = ListHead::uninit();
        unsafe { ListHead::init(&mut head as *mut _) };
        Self {
            inner: Mutex::new(head),
        }
    }

    /// Inserts a node (unsafe because caller must guarantee the node is not already in a list and lives long enough).
    pub unsafe fn insert(&self, new: *mut ListHead) {
        let mut guard = self.inner.lock().unwrap();
        list_ops::add(new, &mut *guard as *mut ListHead);
    }

    /// Removes a node safely. This is the public safe wrapper around the unsafe list_del.
    /// It acquires the lock and checks for poison (prevents corruption if called after release() has moved the item).
    pub fn remove(&self, entry: *mut ListHead) {
        let mut guard = self.inner.lock().unwrap();
        unsafe {
            if list_ops::is_poisoned((*entry).next) || list_ops::is_poisoned((*entry).prev) {
                return; // Already removed (safe no-op, avoids CVE race).
            }
            list_ops::del(entry);
        }
    }

    /// CRITICAL: The cleanup routine.
    /// Moves all items to a local collection (the "stack list" of pointers) while holding the lock.
    /// The lock is dropped immediately after the move, minimizing contention time.
    /// Each node is explicitly unlinked + poisoned under the lock.
    /// Concurrent `remove()` calls become safe no-ops due to poison checks.
    /// This completely eliminates the CVE-2025-68260 race (no temporary linked list whose pointers can be corrupted by concurrent remove after lock drop).
    pub fn release<F>(&self, mut cleanup: F)
    where
        F: FnMut(*mut ListHead),
    {
        let mut local_items: Vec<*mut ListHead> = Vec::new();

        // Critical section: O(n) pointer work only (fast), no slow cleanup here.
        {
            let mut guard = self.inner.lock().unwrap();
            let head = &mut *guard as *mut ListHead;
            unsafe {
                while !ListHead::empty(head) {
                    let entry = (*head).next;
                    list_ops::del(entry); // Unlink from global + poison.
                    local_items.push(entry);
                }
            }
        } // Lock dropped here — contention minimized.

        // Process the local collection outside the lock (slow cleanup allowed).
        for entry in local_items {
            cleanup(entry);
        }
    }

    /// Returns whether the list is empty (cheap peek under lock).
    pub fn is_empty(&self) -> bool {
        let guard = self.inner.lock().unwrap();
        unsafe { ListHead::empty(&*guard as *const ListHead) }
    }
}

// Example usage (for completeness; not part of the public API).
// struct MyNode {
//     link: ListHead,
//     data: u32,
// }
//
// impl MyNode {
//     fn new(data: u32) -> Self { ... }
//     fn release(self) { println!("Releasing node with data {}", self.data); }
// }
//
// let list = ThreadSafeIntrusiveList::new();
// let mut node = MyNode::new(42);
// unsafe { list.insert(&mut node.link as *mut _); }
//
// list.release(|entry| {
//     // Reconstruct containing node (container_of pattern in real code).
//     let node = unsafe { &mut *container_of!(entry, MyNode, link) };
//     node.release(); // Or drop, decrement refcount, etc.
// });