use std::sync::{Arc, Mutex};
use std::ptr;
use std::thread;

/// An intrusive Node. In a real binder-like implementation, 
/// this would be embedded inside a larger struct.
struct Node {
    data: i32,
    prev: *mut Node,
    next: *mut Node,
}

struct IntrusiveList {
    head: *mut Node,
    tail: *mut Node,
}

unsafe impl Send for Node {}
unsafe impl Sync for Node {}

struct ListManager {
    // Standard Mutex to protect the list head/tail pointers
    list: Mutex<IntrusiveList>,
}

impl ListManager {
    fn new() -> Self {
        Self {
            list: Mutex::new(IntrusiveList {
                head: ptr::null_mut(),
                tail: ptr::null_mut(),
            }),
        }
    }

    /// Adds a node to the front of the list (Intrusive)
    fn push_front(&self, data: i32) {
        let new_node = Box::into_raw(Box::new(Node {
            data,
            prev: ptr::null_mut(),
            next: ptr::null_mut(),
        }));

        let mut guard = self.list.lock().unwrap();
        unsafe {
            if !guard.head.is_null() {
                (*guard.head).prev = new_node;
                (*new_node).next = guard.head;
            } else {
                guard.tail = new_node;
            }
            guard.head = new_node;
        }
    }

    /// CVE-2025-68260 Mitigation: The "Release and Localize" pattern.
    /// We minimize lock contention by moving the entire chain to a 
    /// local stack list, then cleaning up outside the lock.
    pub fn release(&self) {
        let mut local_head: *mut Node;

        // --- CRITICAL SECTION ---
        {
            let mut guard = self.list.lock().unwrap();
            if guard.head.is_null() {
                return;
            }

            // Snapshot the head and "empty" the global list immediately.
            // Any concurrent remove() calls will now see an empty list or 
            // null pointers, preventing synchronization races.
            local_head = guard.head;
            guard.head = ptr::null_mut();
            guard.tail = ptr::null_mut();
        } 
        // --- LOCK DROPPED ---

        // Clean up nodes on the local stack. 
        // No other thread can access these pointers now.
        let mut current = local_head;
        while !current.is_null() {
            unsafe {
                let next_node = (*current).next;
                println!("[Cleanup] Dropping node with data: {}", (*current).data);
                
                // Re-take ownership to drop the memory safely
                let _ = Box::from_raw(current);
                current = next_node;
            }
        }
    }
}

fn main() {
    let manager = Arc::new(ListManager::new());

    // Populate the list
    for i in 0..5 {
        manager.push_front(i);
    }

    println!("Starting thread-safe cleanup...");

    let m1 = Arc::clone(&manager);
    let handle = thread::spawn(move || {
        m1.release();
    });

    // Simultaneously attempt a release on another thread 
    // (The first one to the lock gets the list, the second finds it empty)
    let m2 = Arc::clone(&manager);
    m2.release();

    handle.join().unwrap();
    println!("Cleanup complete.");
}