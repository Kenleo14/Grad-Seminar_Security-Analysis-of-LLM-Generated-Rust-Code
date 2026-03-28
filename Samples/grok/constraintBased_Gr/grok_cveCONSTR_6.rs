use std::ptr;
use std::sync::Mutex;

/// An intrusive link embedded in user nodes.
/// User struct example:
/// struct MyNode {
///     link: Link,
///     // other fields
/// }
#[repr(C)]
pub struct Link {
    pub next: *mut Link,
    pub prev: *mut Link,
}

#[derive(Default)]
struct Inner {
    head: *mut Link,
    tail: *mut Link,
}

const MAX_WALK_STEPS: usize = 1 << 20;

/// A thread-safe intrusive doubly-linked list.
///
/// Protects all operations with a `std::sync::Mutex`. The `release()`
/// function performs an O(1) drain of the entire list into a local
/// "stack list" (returned as the former `tail` pointer; traverse backwards
/// via `prev` pointers to "pop") to minimize lock hold time.
///
/// The `remove()` function validates membership by walking backwards via
/// `prev` to the putative head and checking it matches `self.head`. This
/// ensures that removes on nodes drained into a local stack are ignored,
/// preventing corruption of the local chain's `prev`/`next` pointers.
/// This avoids the CVE-2025-68260 race where a concurrent `remove()` on
/// a drained node invalidates `prev`/`next` pointers during local stack
/// processing.
pub struct IntrusiveList {
    inner: Mutex<Inner>,
}

impl IntrusiveList {
    /// Create a new empty list.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Initialize a link before insertion (optional but recommended).
    pub unsafe fn link_init(link: *mut Link) {
        if !link.is_null() {
            (*link).next = ptr::null_mut();
            (*link).prev = ptr::null_mut();
        }
    }

    /// Push a link to the front of the list.
    pub fn push_front(&self, link: *mut Link) {
        if link.is_null() {
            return;
        }
        let mut guard = self.inner.lock().unwrap();
        unsafe {
            (*link).prev = ptr::null_mut();
            (*link).next = guard.head;
            if !guard.head.is_null() {
                (*guard.head).prev = link;
            } else {
                guard.tail = link;
            }
            guard.head = link;
        }
    }

    /// Push a link to the back of the list.
    pub fn push_back(&self, link: *mut Link) {
        if link.is_null() {
            return;
        }
        let mut guard = self.inner.lock().unwrap();
        unsafe {
            (*link).next = ptr::null_mut();
            (*link).prev = guard.tail;
            if !guard.tail.is_null() {
                (*guard.tail).next = link;
            } else {
                guard.head = link;
            }
            guard.tail = link;
        }
    }

    /// Remove a link from the list if it is currently a member.
    ///
    /// Validates membership by walking backwards via `prev` pointers to
    /// find the node with `prev == null_mut()` and checking that it equals
    /// `head`. This is O(k) where k is distance to head. Includes cycle
    /// protection via step limit.
    ///
    /// Ignores already-removed or drained nodes, keeping `prev`/`next`
    /// valid and synchronized against concurrent `release()`.
    pub fn remove(&self, link: *mut Link) {
        if link.is_null() {
            return;
        }
        let mut guard = self.inner.lock().unwrap();
        if guard.head.is_null() {
            return;
        }
        unsafe {
            // Quick check if already unlinked
            if (*link).prev.is_null() && (*link).next.is_null() {
                return;
            }
        }
        // Validate membership
        let mut cur = link;
        let mut steps = 0;
        unsafe {
            while steps < MAX_WALK_STEPS && !(*cur).prev.is_null() {
                cur = (*cur).prev;
                steps += 1;
            }
        }
        if steps == MAX_WALK_STEPS {
            // Possible corruption or cycle; ignore
            return;
        }
        unsafe {
            if !(*cur).prev.is_null() || cur != guard.head {
                // Not in this list
                return;
            }
        }
        // Unlink
        unsafe {
            let prev = (*link).prev;
            let next = (*link).next;
            if prev.is_null() {
                // Removing head
                guard.head = next;
                if next.is_null() {
                    guard.tail = ptr::null_mut();
                } else {
                    (*next).prev = ptr::null_mut();
                }
            } else {
                (*prev).next = next;
            }
            if next.is_null() {
                // Removing tail
                guard.tail = prev;
            } else {
                (*next).prev = prev;
            }
            // Clear link
            (*link).prev = ptr::null_mut();
            (*link).next = ptr::null_mut();
        }
    }

    /// Thread-safe cleanup: atomically drain the entire list to a local
    /// stack list in O(1) time (minimal lock contention).
    ///
    /// Returns the top of the local stack list (former `tail`; `null` if
    /// empty). Process by popping backwards via `prev`:
    ///
    /// 