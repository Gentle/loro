use std::{
    fmt::Display,
    ops::{Deref, DerefMut},
};

use rle::{
    rle_tree::{
        node::{InternalNode, LeafNode, Node},
        tree_trait::{Position, RleTreeTrait},
    },
    HasLength, Mergable, RleTree, Sliceable,
};

#[derive(Debug)]
#[repr(transparent)]
pub struct CustomString(String);
impl Deref for CustomString {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for CustomString {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl HasLength for CustomString {
    fn len(&self) -> usize {
        self.0.len()
    }
}

impl Mergable for CustomString {
    fn is_mergable(&self, other: &Self, _conf: &()) -> bool
    where
        Self: Sized,
    {
        self.len() + other.len() < 64
    }

    fn merge(&mut self, other: &Self, _conf: &())
    where
        Self: Sized,
    {
        self.push_str(other.as_str())
    }
}

impl Sliceable for CustomString {
    fn slice(&self, from: usize, to: usize) -> Self {
        CustomString(self.0[from..to].to_owned())
    }
}

#[derive(Debug)]
pub struct StringTreeTrait;
impl RleTreeTrait<CustomString> for StringTreeTrait {
    const MAX_CHILDREN_NUM: usize = 4;

    const MIN_CHILDREN_NUM: usize = Self::MAX_CHILDREN_NUM / 2;

    type Int = usize;

    type InternalCache = usize;

    type LeafCache = usize;

    fn update_cache_leaf(node: &mut rle::rle_tree::node::LeafNode<'_, CustomString, Self>) {
        node.cache = node.children().iter().map(|x| HasLength::len(&**x)).sum();
    }

    fn update_cache_internal(node: &mut rle::rle_tree::node::InternalNode<'_, CustomString, Self>) {
        node.cache = node.children().iter().map(Node::len).sum();
    }

    fn find_pos_internal(
        node: &mut InternalNode<'_, CustomString, Self>,
        mut index: Self::Int,
    ) -> (usize, Self::Int, Position) {
        let mut last_cache = 0;
        for (i, child) in node.children().iter().enumerate() {
            last_cache = match child {
                Node::Internal(x) => {
                    if index <= x.cache {
                        return (i, index, get_pos(index, child));
                    }
                    x.cache
                }
                Node::Leaf(x) => {
                    if index <= x.cache {
                        return (i, index, get_pos(index, child));
                    }
                    x.cache
                }
            };

            index -= last_cache;
        }

        if index > 0 {
            dbg!(&node);
            assert_eq!(index, 0);
        }
        (node.children().len() - 1, last_cache, Position::End)
    }

    fn find_pos_leaf(
        node: &mut LeafNode<'_, CustomString, Self>,
        mut index: Self::Int,
    ) -> (usize, usize, Position) {
        for (i, child) in node.children().iter().enumerate() {
            if index < HasLength::len(&**child) {
                return (i, index, get_pos(index, &**child));
            }

            index -= HasLength::len(&**child);
        }

        (
            node.children().len() - 1,
            HasLength::len(&**node.children().last().unwrap()),
            Position::End,
        )
    }

    fn len_leaf(node: &LeafNode<'_, CustomString, Self>) -> usize {
        node.cache
    }

    fn len_internal(node: &InternalNode<'_, CustomString, Self>) -> usize {
        node.cache
    }

    fn check_cache_internal(node: &InternalNode<'_, CustomString, Self>) {
        assert_eq!(node.cache, node.children().iter().map(|x| x.len()).sum());
    }

    fn check_cache_leaf(node: &LeafNode<'_, CustomString, Self>) {
        assert_eq!(node.cache, node.children().iter().map(|x| x.len()).sum());
    }
}

fn get_pos<T: HasLength>(index: usize, child: &T) -> Position {
    if index == 0 {
        Position::Start
    } else if index == child.len() {
        Position::End
    } else {
        Position::Middle
    }
}

impl From<String> for CustomString {
    fn from(origin: String) -> Self {
        CustomString(origin)
    }
}

impl From<&str> for CustomString {
    fn from(origin: &str) -> Self {
        CustomString(origin.to_owned())
    }
}
