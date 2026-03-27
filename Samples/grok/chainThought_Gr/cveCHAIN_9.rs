//! Fixed simplified Android Binder Node management system.
//! Addresses CVE-2025-68260 by maintaining the synchronization invariant
//! for the *entire* list-transfer lifecycle (swap + processing).
//! The intrusive list is protected by the mutex at all times; the temp list
//! never escapes the locked section.

use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

/// Death notification entry with embedded intrusive links.
pub struct DeathNotification {
    id: u64,
    callback: Option<Box<dyn FnOnce(u64) + Send>>,
    links: ListLinks,
}

#[derive(Default)]
struct ListLinks {
    next: Option<NonNull<DeathNotification>>,
    prev: Option<NonNull<DeathNotification>>,
}

impl DeathNotification {
    pub fn new(id: u64, callback: impl FnOnce(u64) + Send + 'static) -> Self {
        Self {
            id,
            callback: Some(Box::new(callback)),
            links: ListLinks::default(),
        }
    }
}

struct BinderNodeInner {
    death_list_head: Option<NonNull<DeathNotification>>,
}

pub struct BinderNode {
    inner: Mutex<BinderNodeInner>,
}

impl BinderNode {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(BinderNodeInner {
                death_list_head: None,
            }),
        }
    }

    /// Register a death notification. Ownership transferred to the list.
    pub fn register_death(&self, death: Box<DeathNotification>) {
        let entry = NonNull::new(Box::into_raw(death)).expect("Box::into_raw never returns null");
        let mut guard = self.inner.lock().unwrap();
        add_to_list(&mut guard.death_list_head, entry);
    }

    /// Example unsafe removal path (mirrors rust_binder). In a real system
    /// this would be exposed via a registration token. The safety contract
    /// is now upheld because the release path never drops the lock early.
    pub unsafe fn unregister_death(&self, death: NonNull<DeathNotification>) {
        let mut guard = self.inner.lock().unwrap();
        remove_from_list(&mut guard.death_list_head, death);
    }
}

/// O(1) add to front (same as before).
fn add_to_list(head: &mut Option<NonNull<DeathNotification>>, entry: NonNull<DeathNotification>) {
    unsafe {
        let entry_mut = entry.as_mut();
        entry_mut.links.next = *head;
        entry_mut.links.prev = None;
        if let Some(old) = *head {
            old.as_mut().links.prev = Some(entry);
        }
        *head = Some(entry);
    }
}

/// Unsafe removal by direct pointer surgery (mirrors the CVE-vulnerable code).
/// # Safety
/// - The entry must be in this list or already removed.
/// - The caller must hold the node's lock (invariant now maintained
///   for the full list-transfer lifecycle in `Drop`).
fn remove_from_list(head: &mut Option<NonNull<DeathNotification>>, entry: NonNull<DeathNotification>) {
    unsafe {
        let e = entry.as_mut();
        let prev = e.links.prev;
        let next = e.links.next;

        if let Some(p) = prev {
            p.as_mut().links.next = next;
        } else {
            // Was head
            *head = next;
        }
        if let Some(n) = next {
            n.as_mut().links.prev = prev;
        }
    }
}

/// Process the temporary list. Runs *while the lock is still held*.
fn process_death_notifications(mut head: Option<NonNull<DeathNotification>>) {
    while let Some(node_ptr) = head {
        unsafe {
            let next = node_ptr.as_ref().links.next;
            let node = &mut *node_ptr.as_ptr();
            if let Some(cb) = node.callback.take() {
                cb(node.id);
            }
            let _ = Box::from_raw(node_ptr.as_ptr());
            head = next;
        }
    }
}

impl Drop for BinderNode {
    fn drop(&mut self) {
        let mut guard = self.inner.lock().unwrap();

        // === CRITICAL: List transfer under lock ===
        let mut temp_head = None;
        std::mem::swap(&mut guard.death_list_head, &mut temp_head);
        // ===========================================

        // Process the temporary list *before* the guard drops.
        // This maintains the synchronization invariant for the entire
        // lifecycle of the moved list (no concurrent remove can interleave).
        process_death_notifications(temp_head);

        // Lock released only after processing completes.
    }
}