// intrusive_list_cleanup.rs
// Thread-safe intrusive linked list with safe release/cleanup routine
// Addresses CVE-2025-68260 pattern by keeping synchronization during full list transfer

use std::sync::{Arc, Mutex, Weak};
use std::ptr;
use std::marker::PhantomPinned;
use std::pin::Pin;

/// Embedded intrusive links (mimicking kernel-style intrusive lists).
#[derive(Default)]
struct IntrusiveLink {
    next: *mut IntrusiveLink,
    prev: *mut IntrusiveLink,
}

/// Example payload: DeathNotification (or any list element).
/// In real Binder, this would contain a death recipient, cookie, etc.
#[derive(Default)]
pub struct DeathNotification {
    link: IntrusiveLink,
    /// Weak back-reference to owning node (prevents cycles).
    node: Weak<BinderNode>,
    id: String,
    /// Callback to invoke on cleanup (death notification).
    callback: Box<dyn Fn() + Send + Sync>,
    /// Ensures the element is not moved after insertion (stable address required).
    _pin: PhantomPinned,
}

impl DeathNotification {
    pub fn new(id: String, callback: Box<dyn Fn() + Send + Sync>) -> Self {
        Self {
            id,
            callback,
            ..Default::default()
        }
    }

    pub fn notify(&self) {
        (self.callback)();
    }
}

/// The container managing the intrusive list (BinderNode equivalent).
pub struct BinderNode {
    inner: Mutex<BinderNodeInner>,
}

struct BinderNodeInner {
    is_dead: bool,
    /// Circular intrusive list head.
    death_list_head: IntrusiveLink,
    death_count: usize,
}

impl Default for BinderNodeInner {
    fn default() -> Self {
        let mut head = IntrusiveLink::default();
        // Circular: head points to itself when empty.
        head.next = &mut head as *mut _;
        head.prev = &mut head as *mut _;
        Self {
            is_dead: false,
            death_list_head: head,
            death_count: 0,
        }
    }
}

impl BinderNode {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(BinderNodeInner::default()),
        })
    }

    /// Link a new notification (must be pinned for address stability).
    pub fn link_to_death(self: &Arc<Self>, notification: Pin<Box<DeathNotification>>) -> bool {
        let mut guard = self.inner.lock().unwrap();
        if guard.is_dead {
            return false;
        }

        let notif_ptr = unsafe { &*notification as *const DeathNotification as *mut DeathNotification };
        let link_ptr = unsafe { &mut (*notif_ptr).link as *mut IntrusiveLink };

        // Insert at tail (before head) - O(1).
        unsafe {
            let head = &mut guard.death_list_head as *mut IntrusiveLink;
            let prev = (*head).prev;
            (*link_ptr).next = head;
            (*link_ptr).prev = prev;
            (*prev).next = link_ptr;
            (*head).prev = link_ptr;
        }

        unsafe {
            (*notif_ptr).node = Arc::downgrade(self);
        }
        guard.death_count += 1;
        true
    }

    /// Remove a specific notification by ID (O(n) traversal; real code often uses cookies).
    pub fn remove(&self, id: &str) -> bool {
        let mut guard = self.inner.lock().unwrap();
        if guard.is_dead {
            return false;
        }

        let head = &mut guard.death_list_head as *mut IntrusiveLink;
        let mut current = unsafe { (*head).next };

        while current != head {
            let notif = unsafe {
                &mut *((current as *mut u8)
                    .sub(std::mem::offset_of!(DeathNotification, link)) as *mut DeathNotification)
            };

            if notif.id == id {
                // Unlink - unsafe pointer manipulation.
                unsafe {
                    let next = (*current).next;
                    let prev = (*current).prev;
                    (*prev).next = next;
                    (*next).prev = prev;

                    // Poison links to help detect bugs.
                    (*current).next = ptr::null_mut();
                    (*current).prev = ptr::null_mut();
                }
                guard.death_count -= 1;
                return true;
            }
            current = unsafe { (*current).next };
        }
        false
    }

    /// Thread-safe cleanup routine (release).
    /// 
    /// **CVE-2025-68260 fix**: 
    /// - Full drain + unlink happens **under the lock**.
    /// - Shared list is completely reset **before** guard drops.
    /// - Moved items become exclusively owned by the local temporary collection.
    /// - No `prev`/`next` pointers from drained items remain reachable by concurrent `remove()`.
    /// - Lock contention is minimized: critical section only does pointer work (fast); callbacks run after.
    pub fn release(&self) {
        // Local temporary list on stack (small fixed capacity for common case).
        // This avoids heap allocation in the hot path while satisfying "move to local stack list".
        const STACK_CAP: usize = 16;
        let mut temp_stack: [Option<Box<DeathNotification>>; STACK_CAP] = Default::default();
        let mut stack_idx = 0;
        let mut overflow_vec: Option<Vec<Box<DeathNotification>>> = None;

        {
            let mut guard = self.inner.lock().unwrap();

            if guard.is_dead {
                return; // Idempotent.
            }
            guard.is_dead = true;

            let head = &mut guard.death_list_head as *mut IntrusiveLink;
            let mut current = unsafe { (*head).next };

            while current != head {
                let next = unsafe { (*current).next };

                let notif_ptr = unsafe {
                    (current as *mut u8)
                        .sub(std::mem::offset_of!(DeathNotification, link))
                        as *mut DeathNotification
                };

                // Unlink from shared list (unsafe).
                unsafe {
                    let prev = (*current).prev;
                    (*prev).next = next;
                    (*next).prev = prev;
                }

                // Transfer ownership to local temporary (move from intrusive list).
                let notif_box = unsafe { Box::from_raw(notif_ptr) };

                // Prefer stack storage.
                if stack_idx < STACK_CAP {
                    temp_stack[stack_idx] = Some(notif_box);
                    stack_idx += 1;
                } else {
                    if overflow_vec.is_none() {
                        overflow_vec = Some(Vec::with_capacity(guard.death_count));
                    }
                    overflow_vec.as_mut().unwrap().push(notif_box);
                }

                current = next;
            }

            // Reset shared list to empty (circular head).
            unsafe {
                (*head).next = head;
                (*head).prev = head;
            }
            guard.death_count = 0;
        } // MutexGuard drops here. All shared mutations complete. No race window.

        // Process callbacks outside the lock (now fully safe: exclusive ownership).
        // Stack items first.
        for item in temp_stack.iter_mut().take(stack_idx) {
            if let Some(notif) = item.take() {
                if notif.node.upgrade().is_some() {
                    notif.notify();
                }
                // Drop happens automatically.
            }
        }

        // Overflow items (if any).
        if let Some(vec) = overflow_vec {
            for notif in vec {
                if notif.node.upgrade().is_some() {
                    notif.notify();
                }
            }
        }
    }

    pub fn is_dead(&self) -> bool {
        self.inner.lock().unwrap().is_dead
    }

    pub fn death_count(&self) -> usize {
        self.inner.lock().unwrap().death_count
    }
}

/// Helper to create pinned notification (required for stable address).
pub fn new_pinned_death_notification(
    id: String,
    callback: Box<dyn Fn() + Send + Sync>,
) -> Pin<Box<DeathNotification>> {
    Box::pin(DeathNotification::new(id, callback))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    #[test]
    fn test_basic_release() {
        let node = BinderNode::new();
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();

        let notif = new_pinned_death_notification("test".into(), Box::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }));

        assert!(node.link_to_death(notif));
        assert_eq!(node.death_count(), 1);

        node.release();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(node.is_dead());
    }

    #[test]
    fn test_concurrent_safety() {
        let node = BinderNode::new();
        let barrier = Arc::new(std::sync::Barrier::new(3));

        // Thread performing release.
        let n1 = node.clone();
        let b1 = barrier.clone();
        let release_handle = thread::spawn(move || {
            b1.wait();
            // Add some items then release.
            for i in 0..20 {
                let notif = new_pinned_death_notification(format!("r{}", i), Box::new(|| {}));
                let _ = n1.link_to_death(notif);
            }
            n1.release();
        });

        // Concurrent remover / linker.
        let n2 = node.clone();
        let b2 = barrier.clone();
        thread::spawn(move || {
            b2.wait();
            for _ in 0..100 {
                let _ = n2.remove("nonexistent");
                let notif = new_pinned_death_notification("race".into(), Box::new(|| {}));
                let _ = n2.link_to_death(notif);
                let _ = n2.remove("race");
            }
        });

        barrier.wait();
        release_handle.join().unwrap();
        assert!(node.is_dead());
    }

    #[test]
    fn test_many_notifications_stack_overflow_handling() {
        let node = BinderNode::new();
        for i in 0..50 {
            let notif = new_pinned_death_notification(format!("many{}", i), Box::new(|| {}));
            let _ = node.link_to_death(notif);
        }
        node.release(); // Should handle via overflow_vec without panic.
        assert!(node.is_dead());
    }
}