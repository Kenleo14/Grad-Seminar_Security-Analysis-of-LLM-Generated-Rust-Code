use intrusive_collections::{intrusive_adapter, LinkedList, LinkedListLink, UnsafeRef};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug)]
struct DeathNotification {
    link: LinkedListLink,
    message: String,
}

intrusive_adapter!(DeathNotificationAdapter = UnsafeRef<DeathNotification>: DeathNotification { link: LinkedListLink });

struct BinderNode {
    death_notifications: LinkedList<DeathNotificationAdapter>,
}

impl BinderNode {
    fn new() -> Self {
        BinderNode {
            death_notifications: LinkedList::new(DeathNotificationAdapter::new()),
        }
    }

    fn add_death_notification(&mut self, message: String) {
        let dn = Box::new(DeathNotification {
            link: LinkedListLink::new(),
            message,
        });
        self.death_notifications.push_back(UnsafeRef::from_box(dn));
    }

    fn handle_death_notifications(&mut self) {
        let mut cursor = self.death_notifications.front_mut();
        while let Some(dn) = cursor.get() {
            println!("Handling death notification: {}", dn.message);
            cursor.remove();
        }
    }
}

struct BinderSystem {
    nodes: Arc<Mutex<Vec<BinderNode>>>,
    cleanup_nodes: Arc<Mutex<Vec<BinderNode>>>,
}

impl BinderSystem {
    fn new() -> Self {
        BinderSystem {
            nodes: Arc::new(Mutex::new(Vec::new())),
            cleanup_nodes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn add_node(&self) {
        let node = BinderNode::new();
        self.nodes.lock().unwrap().push(node);
    }

    fn simulate_death(&self, node_index: usize) {
        let mut nodes = self.nodes.lock().unwrap();
        if node_index < nodes.len() {
            let node = nodes.remove(node_index);
            self.cleanup_nodes.lock().unwrap().push(node);
        }
    }

    fn cleanup(&self) {
        let mut cleanup_nodes = self.cleanup_nodes.lock().unwrap();
        for node in cleanup_nodes.drain(..) {
            // Handle death notifications for each node
            node.handle_death_notifications();
        }
    }

    fn concurrent_cleanup(&self) {
        let cleanup_nodes = self.cleanup_nodes.clone();
        thread::spawn(move || {
            let mut nodes = cleanup_nodes.lock().unwrap();
            for node in nodes.drain(..) {
                // Handle death notifications for each node
                node.handle_death_notifications();
            }
        });
    }
}

fn main() {
    let binder_system = BinderSystem::new();

    // Simulate adding nodes
    binder_system.add_node();
    binder_system.add_node();

    // Simulate death notification
    {
        let mut nodes = binder_system.nodes.lock().unwrap();
        if let Some(node) = nodes.get_mut(0) {
            node.add_death_notification("Node 0 failed".to_string());
        }
    }

    // Simulate node death
    binder_system.simulate_death(0);

    // Perform cleanup
    binder_system.cleanup();

    // Alternatively, use concurrent cleanup
    binder_system.concurrent_cleanup();
}