use crossbeam::queue::SegQueue;
use intrusive_collections::{intrusive_adapter, LinkedList, LinkedListLink};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug)]
struct BinderNode {
    link: LinkedListLink,
    id: usize,
    // Other fields related to the node can be added here
}

intrusive_adapter!(BinderNodeAdapter = Arc<BinderNode>: BinderNode { link: LinkedListLink });

struct BinderNodeManager {
    active_nodes: LinkedList<BinderNodeAdapter>,
    cleanup_queue: SegQueue<Arc<BinderNode>>,
}

impl BinderNodeManager {
    fn new() -> Self {
        BinderNodeManager {
            active_nodes: LinkedList::new(BinderNodeAdapter::new()),
            cleanup_queue: SegQueue::new(),
        }
    }

    fn add_node(&mut self, node: Arc<BinderNode>) {
        self.active_nodes.push_back(node);
    }

    fn remove_node(&mut self, node: &Arc<BinderNode>) {
        if self.active_nodes.cursor_mut_from_ptr(node).remove().is_some() {
            self.cleanup_queue.push(node.clone());
        }
    }

    fn process_cleanup(&self) {
        while let Some(node) = self.cleanup_queue.pop() {
            // Perform cleanup
            println!("Cleaning up node: {:?}", node.id);
            // Node will automatically be dropped here if no other references exist
        }
    }
}

fn main() {
    let manager = Arc::new(Mutex::new(BinderNodeManager::new()));

    // Simulate adding and removing nodes concurrently
    let threads: Vec<_> = (0..10)
        .map(|i| {
            let manager = Arc::clone(&manager);
            thread::spawn(move || {
                let node = Arc::new(BinderNode {
                    link: LinkedListLink::new(),
                    id: i,
                });

                {
                    let mut manager = manager.lock().unwrap();
                    manager.add_node(node.clone());
                }

                // Simulate some work with the node
                thread::sleep(std::time::Duration::from_millis(50));

                {
                    let mut manager = manager.lock().unwrap();
                    manager.remove_node(&node);
                }
            })
        })
        .collect();

    for thread in threads {
        thread.join().unwrap();
    }

    // Final cleanup
    let manager = manager.lock().unwrap();
    manager.process_cleanup();
}