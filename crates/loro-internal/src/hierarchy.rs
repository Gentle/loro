use std::{
    fmt::Debug,
    sync::{Arc, Mutex},
};

use fxhash::{FxHashMap, FxHashSet};

use crate::{
    container::{registry::ContainerRegistry, ContainerID},
    event::{Event, EventDispatch, Index, Observer, Path, PathAndTarget, RawEvent, SubscriptionID},
};

/// [`Hierarchy`] stores the hierarchical relationship between containers
#[derive(Default)]
pub struct Hierarchy {
    observers: FxHashMap<SubscriptionID, Observer>,
    nodes: FxHashMap<ContainerID, Node>,
    root_observers: FxHashSet<SubscriptionID>,
    latest_deleted: FxHashSet<ContainerID>,
    event_counter: SubscriptionID,
    deleted_observers: Vec<SubscriptionID>,
    pending_dispatches: Option<(Event, Vec<EventDispatch>)>,
    calling: bool,
}

#[derive(Default)]
struct Node {
    parent: Option<ContainerID>,
    children: FxHashSet<ContainerID>,
    observers: FxHashSet<SubscriptionID>,
    deep_observers: FxHashSet<SubscriptionID>,
}

impl Debug for Hierarchy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hierarchy")
            .field("nodes", &self.nodes)
            .finish()
    }
}

impl Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("parent", &self.parent)
            .field("children", &self.children)
            .field("observers.len", &self.observers.len())
            .field("deep_observers.len", &self.deep_observers.len())
            .finish()
    }
}

impl Hierarchy {
    #[inline(always)]
    pub fn new() -> Self {
        Default::default()
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    // TODO: rename to register?
    pub fn add_child(&mut self, parent: &ContainerID, child: &ContainerID) {
        debug_log::debug_dbg!(&parent, &child);
        let parent_node = self.nodes.entry(parent.clone()).or_default();
        parent_node.children.insert(child.clone());
        let child_node = self.nodes.entry(child.clone()).or_default();
        child_node.parent = Some(parent.clone());
    }

    #[inline(always)]
    pub fn has_children(&self, id: &ContainerID) -> bool {
        self.nodes
            .get(id)
            .map(|node| !node.children.is_empty())
            .unwrap_or(false)
    }

    #[inline(always)]
    pub fn contains(&self, id: &ContainerID) -> bool {
        self.nodes.get(id).is_some() || id.is_root()
    }

    pub fn remove_child(&mut self, parent: &ContainerID, child: &ContainerID) {
        let Some(parent_node) = self.nodes.get_mut(parent) else {
            return;
        };

        parent_node.children.remove(child);
        let mut visited_descendants = FxHashSet::default();
        let mut stack = vec![child];
        while let Some(child) = stack.pop() {
            visited_descendants.insert(child.clone());
            let Some(child_node) = self.nodes.get(child) else {
                continue;
            };
            for child in child_node.children.iter() {
                stack.push(child);
            }
        }

        for descendant in visited_descendants.iter() {
            self.nodes.remove(descendant);
        }

        self.latest_deleted.extend(visited_descendants);
    }

    pub fn take_deleted(&mut self) -> FxHashSet<ContainerID> {
        std::mem::take(&mut self.latest_deleted)
    }

    pub fn get_path_len(&self, id: &ContainerID) -> Option<usize> {
        let mut len = 0;
        let mut current = id;
        while let Some(node) = self.nodes.get(current) {
            len += 1;
            if let Some(parent) = &node.parent {
                current = parent;
            } else {
                break;
            }
        }

        if current.is_root() {
            return Some(len);
        }

        None
    }

    #[inline(always)]
    pub fn get_abs_path(&self, reg: &ContainerRegistry, descendant: &ContainerID) -> Option<Path> {
        let path = self.get_path(reg, descendant, None);
        path.and_then(|x| if x.is_empty() { None } else { Some(x) })
    }

    pub fn get_path(
        &self,
        reg: &ContainerRegistry,
        descendant: &ContainerID,
        target: Option<&ContainerID>,
    ) -> Option<Path> {
        if let ContainerID::Root { name, .. } = descendant {
            return Some(vec![Index::Key(name.into())]);
        }

        if target.map(|x| x == descendant).unwrap_or(false) {
            return Some(vec![]);
        }

        let mut path = Path::default();
        let mut iter_node = Some(descendant);
        while let Some(node_id) = iter_node {
            let Some(node) = self.nodes.get(node_id) else {
                if node_id.is_root() {
                    path.push(Index::Key(node_id.name().into()));
                    break;
                }
                // Deleted node 
                return None;
            };
            let parent = &node.parent;
            if let Some(parent) = parent {
                let parent_node = reg.get(parent).unwrap();
                let index = parent_node
                    .upgrade()
                    .unwrap()
                    .try_lock()
                    .unwrap()
                    .index_of_child(node_id)
                    .unwrap();
                path.push(index);
            } else {
                match node_id {
                    ContainerID::Root {
                        name,
                        container_type: _,
                    } => path.push(Index::Key(name.clone())),
                    _ => return None,
                }
            }

            if parent.as_ref() == target {
                break;
            }

            iter_node = parent.as_ref();
        }

        path.reverse();
        Some(path)
    }

    pub fn should_notify(&self, container_id: &ContainerID) -> bool {
        if !self.root_observers.is_empty() {
            return true;
        }

        let mut node_id = Some(container_id);

        while let Some(inner_node_id) = node_id {
            let Some(node) = self.nodes.get(inner_node_id) else {
                if inner_node_id.is_root() {
                    break;
                } else {
                    // deleted node
                    return false;
                }
            };

            if !node.deep_observers.is_empty() {
                return true;
            }

            node_id = node.parent.as_ref();
        }

        if self
            .nodes
            .get(container_id)
            .map(|x| !x.observers.is_empty())
            .unwrap_or(false)
        {
            return true;
        }

        false
    }

    pub(crate) fn notify_without_lock(hierarchy: Arc<Mutex<Hierarchy>>, raw_event: RawEvent) {
        let target_id = raw_event.container_id;

        let (observers, dispatches, event) = {
            let event = Event {
                absolute_path: raw_event.abs_path,
                relative_path: Default::default(),
                old_version: raw_event.old_version,
                new_version: raw_event.new_version,
                current_target: Some(target_id.clone()),
                target: target_id.clone(),
                diff: raw_event.diff,
                local: raw_event.local,
            };
            let mut dispatches = Vec::new();
            let mut hierarchy = hierarchy.try_lock().unwrap();
            let mut current_target_id = Some(target_id.clone());
            let mut count = 0;
            let mut path_to_root = event.absolute_path.clone();
            path_to_root.reverse();
            let node = hierarchy.nodes.entry(target_id).or_default();
            if !node.observers.is_empty() {
                let mut dispatch = EventDispatch::default();
                for &sub_id in node.observers.iter() {
                    dispatch.sub_ids.push(sub_id);
                }
                dispatches.push(dispatch);
            }
            while let Some(id) = current_target_id {
                let Some(node) = hierarchy.nodes.get_mut(&id) else {
                    break;
                };
                if !node.deep_observers.is_empty() {
                    let mut dispatch = EventDispatch::default();
                    let mut relative_path = path_to_root[..count].to_vec();
                    relative_path.reverse();
                    dispatch.rewrite = Some(PathAndTarget {
                        relative_path,
                        target: Some(id.clone()),
                    });
                    for &sub_id in node.deep_observers.iter() {
                        dispatch.sub_ids.push(sub_id);
                    }
                    dispatches.push(dispatch);
                }

                count += 1;
                if node.parent.is_none() {
                    debug_assert!(id.is_root());
                }

                current_target_id = node.parent.as_ref().cloned();
            }

            if !hierarchy.root_observers.is_empty() {
                debug_log::debug_log!("notify root");
                let mut dispatch = EventDispatch {
                    sub_ids: Default::default(),
                    rewrite: Some(PathAndTarget {
                        relative_path: event.absolute_path.clone(),
                        target: None,
                    }),
                };
                for &sub_id in hierarchy.root_observers.iter() {
                    dispatch.sub_ids.push(sub_id);
                }
                dispatches.push(dispatch);
            }
            if hierarchy.calling {
                hierarchy.pending_dispatches = Some((event, std::mem::take(&mut dispatches)));
                (None, None, None)
            } else {
                hierarchy.calling = true;
                let observers = std::mem::take(&mut hierarchy.observers);
                (Some(observers), Some(dispatches), Some(event))
            }
        };
        if let (Some(mut observers), Some(dispatches), Some(event)) = (observers, dispatches, event)
        {
            Self::call_observers(&mut observers, dispatches, event);
            Self::reset(hierarchy, observers);
        }
    }

    #[inline]
    fn call_observers(
        observers: &mut FxHashMap<SubscriptionID, Observer>,
        dispatches: Vec<EventDispatch>,
        mut event: Event,
    ) {
        for dispatch in dispatches {
            if let Some(PathAndTarget {
                relative_path,
                target,
            }) = dispatch.rewrite
            {
                event.relative_path = relative_path;
                event.current_target = target;
            };
            for sub_id in dispatch.sub_ids.iter() {
                let observer = observers.get_mut(sub_id).unwrap();
                observer(&event);
            }
        }
    }

    #[inline]
    fn reset(hierarchy: Arc<Mutex<Hierarchy>>, mut observers: FxHashMap<SubscriptionID, Observer>) {
        let mut hierarchy_guard = hierarchy.try_lock().unwrap();
        let deleted_ids = std::mem::take(&mut hierarchy_guard.deleted_observers);
        for sub_id in deleted_ids.iter() {
            observers.remove(sub_id);
        }
        hierarchy_guard.observers.extend(observers);
        let pending_dispatches = hierarchy_guard.pending_dispatches.take();
        if let Some((event, dispatches)) = pending_dispatches {
            let mut observers = std::mem::take(&mut hierarchy_guard.observers);
            drop(hierarchy_guard);
            Self::call_observers(&mut observers, dispatches, event);
            Self::reset(hierarchy, observers);
        } else {
            hierarchy_guard.calling = false;
        }
    }

    pub fn subscribe(
        &mut self,
        container: &ContainerID,
        observer: Observer,
        deep: bool,
    ) -> SubscriptionID {
        let id = self.next_id();
        if deep {
            self.nodes
                .entry(container.clone())
                .or_default()
                .deep_observers
                .insert(id);
            self.observers.insert(id, observer);
        } else {
            self.nodes
                .entry(container.clone())
                .or_default()
                .observers
                .insert(id);
            self.observers.insert(id, observer);
        }
        id
    }

    fn next_id(&mut self) -> SubscriptionID {
        let ans = self.event_counter;
        self.event_counter += 1;
        ans
    }

    pub fn unsubscribe(&mut self, container: &ContainerID, id: SubscriptionID) -> bool {
        if let Some(x) = self.nodes.get_mut(container) {
            x.observers
                .remove(&id)
                .then(|| {
                    if self.calling {
                        self.deleted_observers.push(id);
                    }
                })
                .is_some()
        } else {
            // TODO: warning
            false
        }
    }

    pub fn subscribe_root(&mut self, observer: Observer) -> SubscriptionID {
        let id = self.next_id();
        self.root_observers.insert(id);
        self.observers.insert(id, observer);
        id
    }

    pub fn unsubscribe_root(&mut self, sub: SubscriptionID) -> bool {
        self.root_observers
            .remove(&sub)
            .then(|| {
                if self.calling {
                    self.deleted_observers.push(sub);
                }
            })
            .is_some()
    }

    pub fn send_notifications_without_lock(
        hierarchy: Arc<Mutex<Hierarchy>>,
        events: Vec<RawEvent>,
    ) {
        for event in events {
            Hierarchy::notify_without_lock(hierarchy.clone(), event);
        }
    }
}