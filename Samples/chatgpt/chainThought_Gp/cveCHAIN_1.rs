use std::sync::{Arc, Mutex};
use std::ptr;

struct Node {
    data: i32,
    next: *mut Node,
    prev: *mut Node,
}

impl Node {
    fn new(data: i32) -> Self {
        Node {
            data,
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        }
    }
}

struct IntrusiveList {
    head: *mut Node,
    tail: *mut Node,
    lock: Mutex<()>,
}

impl IntrusiveList {
    fn new() -> Self {
        IntrusiveList {
            head: ptr::null_mut(),
            tail: ptr::null_mut(),
            lock: Mutex::new(()),
        }
    }

    fn push(&mut self, node: *mut Node) {
        unsafe {
            let _guard = self.lock.lock().unwrap();
            if self.tail.is_null() {
                self.head = node;
                self.tail = node;
            } else {
                (*node).prev = self.tail;
                (*self.tail).next = node;
                self.tail = node;
            }
        }
    }

    fn transfer_to_stack(&mut self) -> Vec<Arc<Node>> {
        let _guard = self.lock.lock().unwrap();
        let mut stack = Vec::new();
        let mut current = self.head;

        while !current.is_null() {
            unsafe {
                let next = (*current).next;

                // Detach the node from the list
                if !(*current).prev.is_null() {
                    (*(*current).prev).next = next;
                } else {
                    self.head = next;
                }

                if !next.is_null() {
                    (*next).prev = (*current).prev;
                } else {
                    self.tail = (*current).prev;
                }

                // Move node to stack
                stack.push(Arc::from_raw(current));

                current = next;
            }
        }

        stack
    }
}

fn main() {
    let mut list = IntrusiveList::new();
    let node1 = Box::into_raw(Box::new(Node::new(1)));
    let node2 = Box::into_raw(Box::new(Node::new(2)));
    let node3 = Box::into_raw(Box::new(Node::new(3)));

    list.push(node1);
    list.push(node2);
    list.push(node3);

    let stack = list.transfer_to_stack();

    // Do something with the stack
    for node in stack {
        println!("{}", node.data);
    }
}