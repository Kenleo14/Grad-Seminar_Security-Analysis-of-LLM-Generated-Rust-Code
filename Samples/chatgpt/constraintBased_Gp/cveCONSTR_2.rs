use std::sync::{Arc, Mutex};
use std::ptr;

// Define a struct for the node in the intrusive linked list.
struct Node {
    data: i32,
    next: *mut Node,
    prev: *mut Node,
}

impl Node {
    fn new(data: i32) -> *mut Node {
        Box::into_raw(Box::new(Node {
            data,
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        }))
    }
}

// Define the linked list itself.
struct IntrusiveLinkedList {
    head: *mut Node,
    tail: *mut Node,
    lock: Mutex<()>,
}

impl IntrusiveLinkedList {
    fn new() -> Self {
        IntrusiveLinkedList {
            head: ptr::null_mut(),
            tail: ptr::null_mut(),
            lock: Mutex::new(()),
        }
    }

    // Add a node to the list.
    fn append(&mut self, node: *mut Node) {
        unsafe {
            let _lock = self.lock.lock().unwrap();
            if self.tail.is_null() {
                // List is empty.
                self.head = node;
                self.tail = node;
            } else {
                // Append to the end.
                (*node).prev = self.tail;
                (*self.tail).next = node;
                self.tail = node;
            }
        }
    }

    // Release nodes safely.
    fn release(&mut self) -> Vec<*mut Node> {
        let _lock = self.lock.lock().unwrap();
        let mut stack = Vec::new();

        unsafe {
            let mut current = self.head;
            while !current.is_null() {
                stack.push(current);
                let next = (*current).next;

                // Detach current node from the list.
                (*current).next = ptr::null_mut();
                (*current).prev = ptr::null_mut();

                current = next;
            }
            self.head = ptr::null_mut();
            self.tail = ptr::null_mut();
        }

        stack
    }

    // Remove a node from the list.
    fn remove(&mut self, node: *mut Node) {
        let _lock = self.lock.lock().unwrap();

        unsafe {
            if node.is_null() {
                return;
            }

            let prev = (*node).prev;
            let next = (*node).next;

            if !prev.is_null() {
                (*prev).next = next;
            } else {
                // Node is head.
                self.head = next;
            }

            if !next.is_null() {
                (*next).prev = prev;
            } else {
                // Node is tail.
                self.tail = prev;
            }

            // Nullify the node's pointers.
            (*node).next = ptr::null_mut();
            (*node).prev = ptr::null_mut();
        }
    }
}

fn main() {
    let list = Arc::new(Mutex::new(IntrusiveLinkedList::new()));

    {
        let mut list = list.lock().unwrap();
        let node1 = Node::new(1);
        let node2 = Node::new(2);
        list.append(node1);
        list.append(node2);

        // Imagine a concurrent operation that might call remove here.

        // Now safely release the list.
        let released_nodes = list.release();
        for node in released_nodes {
            unsafe {
                println!("Released node with data: {}", (*node).data);
                // Cleanup the node.
                Box::from_raw(node);
            }
        }
    }
}