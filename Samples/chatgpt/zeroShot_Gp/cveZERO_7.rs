use std::sync::{Arc, Mutex, RwLock};
use std::ptr::NonNull;
use std::thread;
use std::collections::LinkedList;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug)]
struct DeathNotification {
    id: usize,
    message: String,
}

struct Node {
    notification: DeathNotification,
    next: Option<NonNull<Node>>,
    prev: Option<NonNull<Node>>,
}

impl Node {
    fn new(notification: DeathNotification) -> Self {
        Node {
            notification,
            next: None,
            prev: None,
        }
    }
}

struct BinderNodeManager {
    head: Option<NonNull<Node>>,
    tail: Option<NonNull<Node>>,
    lock: RwLock<()>,
    cleanup_list: Mutex<LinkedList<NonNull<Node>>>,
}

impl BinderNodeManager {
    fn new() -> Self {
        BinderNodeManager {
            head: None,
            tail: None,
            lock: RwLock::new(()),
            cleanup_list: Mutex::new(LinkedList::new()),
        }
    }

    fn add_notification(&mut self, notification: DeathNotification) {
        let mut node = Box::new(Node::new(notification));
        let node_ptr = unsafe { NonNull::new_unchecked(Box::into_raw(node)) };

        let _write_guard = self.lock.write().unwrap();

        if let Some(tail) = self.tail {
            unsafe {
                tail.as_mut().next = Some(node_ptr);
                node_ptr.as_mut().prev = Some(tail);
            }
        } else {
            self.head = Some(node_ptr);
        }

        self.tail = Some(node_ptr);
    }

    fn remove_node(&mut self, node: NonNull<Node>) {
        let _write_guard = self.lock.write().unwrap();

        unsafe {
            if let Some(prev) = node.as_ref().prev {
                prev.as_mut().next = node.as_ref().next;
            } else {
                self.head = node.as_ref().next;
            }

            if let Some(next) = node.as_ref().next {
                next.as_mut().prev = node.as_ref().prev;
            } else {
                self.tail = node.as_ref().prev;
            }
        }

        self.cleanup_list.lock().unwrap().push_back(node);
    }

    fn cleanup(&self) {
        let mut cleanup_list = self.cleanup_list.lock().unwrap();

        while let Some(node) = cleanup_list.pop_front() {
            unsafe {
                Box::from_raw(node.as_ptr());
            }
        }
    }
}

fn main() {
    let manager = Arc::new(Mutex::new(BinderNodeManager::new()));

    let handles: Vec<_> = (0..10).map(|i| {
        let manager = Arc::clone(&manager);
        thread::spawn(move || {
            let notification = DeathNotification { id: i, message: format!("Node {}", i) };
            manager.lock().unwrap().add_notification(notification);
        })
    }).collect();

    for handle in handles {
        handle.join().unwrap();
    }

    // Simulate concurrent cleanup
    let manager_clone = Arc::clone(&manager);
    thread::spawn(move || {
        manager_clone.lock().unwrap().cleanup();
    }).join().unwrap();
}