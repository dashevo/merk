use super::hash::Hash;
use super::Tree;

// TODO: optimize memory footprint
pub enum Link {
    Pruned {
        hash: Hash,
        height: u8,
        key: Vec<u8>
    },
    Modified {
        pending_writes: usize,
        height: u8,
        tree: Tree
    },
    Stored {
        hash: Hash,
        height: u8,
        tree: Tree
    }
}

impl Link {
    #[inline]
    pub fn from_modified_tree(tree: Tree) -> Self {
        let mut pending_writes = 1
            + tree.child_pending_writes(true)
            + tree.child_pending_writes(false);

        Link::Modified {
            pending_writes,
            height: tree.height(),
            tree
        }
    }

    pub fn maybe_from_modified_tree(maybe_tree: Option<Tree>) -> Option<Self> {
        maybe_tree.map(|tree| Link::from_modified_tree(tree))
    }

    #[inline]
    pub fn is_pruned(&self) -> bool {
        match self {
            Link::Pruned { .. } => true,
            _ => false
        }
    }

    #[inline]
    pub fn is_modified(&self) -> bool {
        match self {
            Link::Modified { .. } => true,
            _ => false
        }
    }

    #[inline]
    pub fn is_stored(&self) -> bool {
        match self {
            Link::Stored { .. } => true,
            _ => false
        }
    }

    pub fn tree(&self) -> Option<&Tree> {
        match self {
            // TODO: panic for Pruned, don't return Option?
            Link::Pruned { .. } => None,
            Link::Modified { tree, .. } => Some(tree),
            Link::Stored { tree, .. } => Some(tree)
        }
    }

    pub fn hash(&self) -> &Hash {
        match self {
            Link::Modified { .. } => panic!("Cannot get hash from modified link"),
            Link::Pruned { hash, .. } => hash,
            Link::Stored { hash, .. } => hash
        }
    }

    pub fn height(&self) -> u8 {
        match self {
            Link::Pruned { height, .. } => *height,
            Link::Modified { height, .. } => *height,
            Link::Stored { height, .. } => *height
        }
    }

    pub fn to_pruned(self) -> Self {
        match self {
            Link::Pruned { .. } => self,
            Link::Modified { .. } => panic!("Cannot prune Modified tree"),
            Link::Stored { hash, height, tree } => Link::Pruned {
                hash,
                height,
                key: tree.take_key()
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use super::super::Tree;
    use super::super::hash::NULL_HASH;
    
    #[test]
    fn from_modified_tree() {
        let tree = Tree::new(vec![0], vec![1]);
        let link = Link::from_modified_tree(tree);
        assert!(link.is_modified());
        assert_eq!(link.height(), 1);
        assert_eq!(link.tree().expect("expected tree").key(), &[0]);
        if let Link::Modified { pending_writes, .. } = link {
            assert_eq!(pending_writes, 1);
        } else {
            panic!("Expected Link::Modified");
        }
    }

    #[test]
    fn maybe_from_modified_tree() {
        let link = Link::maybe_from_modified_tree(None);
        assert!(link.is_none());

        let tree = Tree::new(vec![0], vec![1]);
        let link = Link::maybe_from_modified_tree(Some(tree));
        assert!(link.expect("expected link").is_modified());
    }

    #[test]
    fn types() {
        let hash = NULL_HASH;
        let height = 1;
        let pending_writes = 1;
        let key = vec![0];
        let tree = || Tree::new(vec![0], vec![1]);

        let pruned = Link::Pruned { hash, height, key };
        let modified = Link::Modified { pending_writes, height, tree: tree() };
        let stored = Link::Stored { hash, height, tree: tree() };

        assert!(pruned.is_pruned());
        assert!(!pruned.is_modified());
        assert!(!pruned.is_stored());
        assert!(pruned.tree().is_none());
        assert_eq!(pruned.hash(), &[0; 20]);
        assert_eq!(pruned.height(), 1);
        assert!(pruned.to_pruned().is_pruned());

        assert!(!modified.is_pruned());
        assert!(modified.is_modified());
        assert!(!modified.is_stored());
        assert!(modified.tree().is_some());
        assert_eq!(modified.height(), 1);

        assert!(!stored.is_pruned());
        assert!(!stored.is_modified());
        assert!(stored.is_stored());
        assert!(stored.tree().is_some());
        assert_eq!(stored.hash(), &[0; 20]);
        assert_eq!(stored.height(), 1);
        assert!(stored.to_pruned().is_pruned());
    }

    #[test]
    #[should_panic]
    fn modified_hash() {
        Link::Modified {
            pending_writes: 1,
            height: 1,
            tree: Tree::new(vec![0], vec![1])
        }.hash();
    }

    #[test]
    #[should_panic]
    fn modified_to_pruned() {
        Link::Modified {
            pending_writes: 1,
            height: 1,
            tree: Tree::new(vec![0], vec![1])
        }.to_pruned();
    }
}