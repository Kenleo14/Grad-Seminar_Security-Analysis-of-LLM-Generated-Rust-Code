use std::sync::{Arc, Mutex, Condvar};
use std::ptr::{null_mut, NonNull};
use std::marker::PhantomData;
use std::thread;

// Define a structure for a Node in the intrusive linked list.
struct ListNode {
    prev: *mut ListNode,
    next: *mut ListNode,
    data: i32, // Placeholder for actual node data
}

// Safe abstraction over the raw pointers for ListNode.
struct List {
    head: *mut ListNode,
    tail: *mut ListNode,
}

impl List {
    fn new() -> Self {
        List {
            head: null_mut(),
            tail: null_mut(),
        }
    }

    unsafe fn push(&mut self, node: *mut ListNode) {
        if self.head.is_null() {
            self.head = node;
            self.tail = node;
        } else {
            (*self.tail).next = node;
            (*node).prev = self.tail;
            self.tail = node;
        }
    }

    unsafe fn pop(&mut self) -> Option<*mut ListNode> {
        if self.head.is_null() {
            None
        } else {
            let node = self.head;
            self.head = (*node).next;
            if !self.head.is_null() {
                (*self.head).prev = null_mut();
            } else {
                self.tail = null_mut();
            }
            Some(node)
        }
    }
}

// This represents a single death notification, simplifying the actual Android Binder structure.
struct DeathNotification {
    node: ListNode,
}

struct BinderNodeList {
    list: List,
    lock: Mutex<()>,
    condvar: Condvar,
}

impl BinderNodeList {
    fn new() -> Self {
        BinderNodeList {
            list: List::new(),
            lock: Mutex::new(()),
            condvar: Condvar::new(),
        }
    }

    fn add_notification(&self, notification: Arc<DeathNotification>) {
        let _guard = self.lock.lock().unwrap();
        unsafe {
            let node_ptr = Arc::into_raw(notification) as *mut ListNode;
            self.list.push(node_ptr);
        }
        self.condvar.notify_one();
    }

    fn process_notifications(&self) {
        loop {
            let node_ptr = {
                let _guard = self.lock.lock().unwrap();
                unsafe { self.list.pop() }
            };

            if let Some(node_ptr) = node_ptr {
                unsafe {
                    let notification = Arc::from_raw(node_ptr as *mut DeathNotification);
                    // Process the notification
                    println!("Processing death notification with data: {}", notification.node.data);
                }
            } else {
                // Wait for a new notification
                let _guard = self.lock.lock().unwrap();
                self.condvar.wait(_guard).unwrap();
            }
        }
    }
}

fn main() {
    let binder_node_list = Arc::new(BinderNodeList::new());

    // Simulate adding notifications from different threads
    for i in 0..10 {
        let list_clone = Arc::clone(&binder_node_list);
        thread::spawn(move || {
            let notification = Arc::new(DeathNotification {
                node: ListNode {
                    prev: null_mut(),
                    next: null_mut(),
                    data: i,
                },
            });
            list_clone.add_notification(notification);
        });
    }

    // Simulate processing notifications in another thread
    let list_clone = Arc::clone(&binder_node_list);
    thread::spawn(move || {
        list_clone.process_notifications();
    });

    // Wait some time to see the processing in action
    thread::sleep(std::time::Duration::from_secs(5));
}