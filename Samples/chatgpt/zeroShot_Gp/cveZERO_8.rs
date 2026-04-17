use std::sync::{Arc, Mutex, RwLock};
use std::ptr::{self, NonNull};
use std::thread;
use std::sync::atomic::{AtomicBool, Ordering};

struct DeathNotification {
    // Simulate a death notification structure
    message: String,
}

struct Node {
    death_notification: DeathNotification,
    next: Option<NonNull<Node>>,
    prev: Option<NonNull<Node>>,
    in_list: AtomicBool, // To track if the node is in the list
}

impl Node {
    fn new(message: &str) -> Node {
        Node {
            death_notification: DeathNotification {
                message: message.to_string(),
            },
            next: None,
            prev: None,
            in_list: AtomicBool::new(true),
        }
    }
}

struct IntrusiveLinkedList {
    head: Option<NonNull<Node>>,
    tail: Option<NonNull<Node>>,
}

impl IntrusiveLinkedList {
    fn new() -> Self {
        IntrusiveLinkedList { head: None, tail: None }
    }

    // Add a node to the list
    fn push(&mut self, node: &mut Node) {
        unsafe {
            node.next = None;
            node.prev = self.tail;

            let node_ptr = NonNull::new(node).expect("Node pointer should not be null");

            if let Some(mut tail) = self.tail {
                tail.as_mut().next = Some(node_ptr);
            }

            self.tail = Some(node_ptr);

            if self.head.is_none() {
                self.head = Some(node_ptr);
            }
        }
    }

    // Remove a node from the list
    fn remove(&mut self, node: &mut Node) {
        unsafe {
            if let Some(mut prev) = node.prev {
                prev.as_mut().next = node.next;
            } else {
                self.head = node.next;
            }

            if let Some(mut next) = node.next {
                next.as_mut().prev = node.prev;
            } else {
                self.tail = node.prev;
            }

            node.in_list.store(false, Ordering::SeqCst);
        }
    }
}

struct BinderNodeManager {
    list: RwLock<IntrusiveLinkedList>,
}

impl BinderNodeManager {
    fn new() -> Self {
        BinderNodeManager {
            list: RwLock::new(IntrusiveLinkedList::new()),
        }
    }

    // Add a node to the manager
    fn add_node(&self, node: &mut Node) {
        let mut list = self.list.write().unwrap();
        list.push(node);
    }

    // Simulate a high-concurrency release
    fn release_node(&self, node: &mut Node) {
        {
            let mut list = self.list.write().unwrap();
            list.remove(node);
        }
        // Move to a temporary list for cleanup
        let temp_list = Arc::new(Mutex::new(IntrusiveLinkedList::new()));
        {
            let mut temp_list = temp_list.lock().unwrap();
            temp_list.push(node);
        }
        // Simulate cleanup in a separate thread
        let temp_list_clone = Arc::clone(&temp_list);
        thread::spawn(move || {
            let mut temp_list = temp_list_clone.lock().unwrap();
            // Logic for cleaning up the node
            while let Some(node_ptr) = temp_list.head {
                unsafe {
                    let node = node_ptr.as_ref();
                    println!("Cleaning up node with message: {}", node.death_notification.message);
                    temp_list.remove(node as *const _ as *mut Node);
                }
            }
        });
    }
}

fn main() {
    let manager = BinderNodeManager::new();

    let mut node1 = Node::new("Node 1 died");
    let mut node2 = Node::new("Node 2 died");

    manager.add_node(&mut node1);
    manager.add_node(&mut node2);

    manager.release_node(&mut node1);
    manager.release_node(&mut node2);

    // Give some time for cleanup threads to run
    thread::sleep(std::time::Duration::from_millis(100));
}