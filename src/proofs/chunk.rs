use failure::bail;
use rocksdb::DBRawIterator;

use super::{
    verify::{execute, Tree as ProofTree},
    Node, Op,
};
use crate::error::Result;
use crate::tree::{Fetch, Hash, RefWalker, Tree};

impl<'a, S> RefWalker<'a, S>
where
    S: Fetch + Sized + Send + Clone,
{
    pub(crate) fn create_trunk_proof(&mut self) -> Result<Vec<Op>> {
        let approx_size = 2u8.pow((self.tree().height() / 2) as u32);
        let mut proof = Vec::with_capacity(approx_size as usize);

        let trunk_height = self.traverse_for_height_proof(&mut proof, 1)?;
        self.traverse_for_trunk(&mut proof, trunk_height, true)?;

        Ok(proof)
    }

    // traverse to leftmost node to prove height of tree
    fn traverse_for_height_proof(&mut self, proof: &mut Vec<Op>, depth: usize) -> Result<usize> {
        let maybe_left = self.walk(true)?;
        let has_left_child = maybe_left.is_some();

        let trunk_height = if let Some(mut left) = maybe_left {
            left.traverse_for_height_proof(proof, depth + 1)?
        } else {
            depth / 2
        };

        if depth > trunk_height {
            proof.push(Op::Push(self.to_kvhash_node()));

            if has_left_child {
                proof.push(Op::Parent);
            }

            if let Some(right) = self.walk(false)? {
                proof.push(Op::Push(right.to_hash_node()));
                proof.push(Op::Child);
            }
        }

        Ok(trunk_height)
    }

    // build proof for all nodes in chunk
    fn traverse_for_trunk(
        &mut self,
        proof: &mut Vec<Op>,
        remaining_depth: usize,
        is_leftmost: bool,
    ) -> Result<()> {
        if remaining_depth == 0 {
            // return early if we have reached bottom of trunk

            // connect to hash of left child
            // for leftmost node, we already have height proof
            if !is_leftmost {
                if let Some(left_child) = self.tree().link(true) {
                    proof.push(Op::Push(Node::Hash(*left_child.hash())));
                }
            }

            // add this node's data
            proof.push(Op::Push(self.to_kv_node()));

            // add parent op to connect left child
            if let Some(_) = self.tree().link(true) {
                proof.push(Op::Parent);
            }

            // connect to hash of right child
            if let Some(right_child) = self.tree().link(false) {
                proof.push(Op::Push(Node::Hash(*right_child.hash())));
                proof.push(Op::Child);
            }

            return Ok(());
        }

        // traverse left, guaranteed to have child
        let mut left = self.walk(true)?.unwrap();
        left.traverse_for_trunk(proof, remaining_depth - 1, is_leftmost)?;

        // add this node's data
        proof.push(Op::Push(self.to_kv_node()));
        proof.push(Op::Parent);

        // traverse right, guaranteed to have child
        let mut right = self.walk(false)?.unwrap();
        right.traverse_for_trunk(proof, remaining_depth - 1, false)?;
        proof.push(Op::Child);

        Ok(())
    }
}

pub(crate) fn get_next_chunk(iter: &mut DBRawIterator, end_key: Option<&[u8]>) -> Result<Vec<Op>> {
    let mut chunk = Vec::with_capacity(512);
    let mut stack = Vec::with_capacity(32);
    let mut node = Tree::new(vec![], vec![]);

    while iter.valid() {
        let key = iter.key().unwrap();

        if let Some(end_key) = end_key {
            if key == end_key {
                break;
            }
        }

        let encoded_node = iter.value().unwrap();
        Tree::decode_into(&mut node, vec![], encoded_node);

        let kv = Node::KV(key.to_vec(), node.value().to_vec());
        chunk.push(Op::Push(kv));

        if node.link(true).is_some() {
            chunk.push(Op::Parent);
        }

        if let Some(child) = node.link(false) {
            stack.push(child.key().to_vec());
        } else {
            while let Some(top_key) = stack.last() {
                if key < top_key.as_slice() {
                    break;
                }
                stack.pop();
                chunk.push(Op::Child);
            }
        }

        iter.next();
    }

    Ok(chunk)
}

pub(crate) fn verify_leaf<I: Iterator<Item = Result<Op>>>(
    ops: I,
    expected_hash: Hash,
) -> Result<ProofTree> {
    let tree = execute(ops, false, |node| match node {
        Node::KV(_, _) => Ok(()),
        _ => bail!("Leaf chunks must contain full subtree"),
    })?;

    if tree.hash() != expected_hash {
        bail!(
            "Leaf chunk proof did not match expected hash\n\tExpected: {:?}\n\tActual: {:?}",
            expected_hash,
            tree.hash()
        );
    }

    return Ok(tree);
}

fn verify_trunk<I: Iterator<Item = Result<Op>>>(ops: I) -> Result<ProofTree> {
    fn verify_height_proof(tree: &ProofTree) -> Result<usize> {
        Ok(match tree.child(true) {
            Some(child) => {
                if let Node::Hash(_) = child.tree.node {
                    bail!("Expected height proof to only contain KV and KVHash nodes")
                }
                verify_height_proof(&child.tree)? + 1
            }
            None => 1,
        })
    }

    fn verify_completeness(tree: &ProofTree, remaining_depth: usize, leftmost: bool) -> Result<()> {
        let recurse = |left, leftmost| match tree.child(left) {
            Some(child) => verify_completeness(&child.tree, remaining_depth - 1, left && leftmost),
            None => bail!("Trunk is missing expected nodes"),
        };

        if remaining_depth > 0 {
            match tree.node {
                Node::KV(_, _) => {}
                _ => bail!("Expected trunk inner nodes to contain keys and values"),
            }
            recurse(true, leftmost)?;
            recurse(false, false)?;
        } else if !leftmost {
            match tree.node {
                Node::Hash(_) => {}
                _ => bail!("Expected trunk leaves to contain Hash nodes"),
            }
        } else if leftmost {
            match tree.node {
                Node::KVHash(_) => {}
                _ => bail!("Expected leftmost trunk leaf to contain KVHash node"),
            }
        }

        Ok(())
    }

    let tree = execute(ops, false, |_| Ok(()))?;

    let height = verify_height_proof(&tree)?;
    let expected_depth = height / 2;
    verify_completeness(&tree, expected_depth, true)?;

    Ok(tree)
}

#[cfg(test)]
mod tests {
    use std::usize;

    use super::super::verify::Tree;
    use super::*;
    use crate::test_utils::*;
    use crate::tree::PanicSource;

    #[derive(Default)]
    struct NodeCounts {
        hash: usize,
        kvhash: usize,
        kv: usize,
    }

    fn count_node_types(tree: Tree) -> NodeCounts {
        let mut counts = NodeCounts::default();

        tree.visit_nodes(&mut |node| {
            match node {
                Node::Hash(_) => counts.hash += 1,
                Node::KVHash(_) => counts.kvhash += 1,
                Node::KV(_, _) => counts.kv += 1,
            };
        });

        counts
    }

    #[test]
    fn trunk_roundtrip() {
        let mut tree = make_tree_seq(31);
        let mut walker = RefWalker::new(&mut tree, PanicSource {});

        let proof = walker.create_trunk_proof().unwrap();
        let trunk = verify_trunk(proof.into_iter().map(|op| Ok(op))).unwrap();

        let counts = count_node_types(trunk);
        // counted based on the deterministic structure of this 31-node tree
        assert_eq!(counts.hash, 9);
        assert_eq!(counts.kv, 7);
        assert_eq!(counts.kvhash, 3);
    }

    #[test]
    fn leaf_chunk_roundtrip() {
        let mut merk = TempMerk::new().unwrap();
        let batch = make_batch_seq(0..31);
        merk.apply(batch.as_slice(), &[]).unwrap();

        let root_node = merk.tree.take();
        let root_key = root_node.as_ref().unwrap().key().to_vec();
        merk.tree.set(root_node);

        // whole tree as 1 leaf
        let mut iter = merk.db.raw_iterator();
        iter.seek_to_first();
        let chunk = get_next_chunk(&mut iter, None).unwrap();
        let ops = chunk.into_iter().map(|op| Ok(op));
        let chunk = verify_leaf(ops, merk.root_hash()).unwrap();
        let counts = count_node_types(chunk);
        assert_eq!(counts.kv, 31);
        assert_eq!(counts.hash, 0);
        assert_eq!(counts.kvhash, 0);
        drop(iter);

        let mut iter = merk.db.raw_iterator();
        iter.seek_to_first();

        // left leaf
        let chunk = get_next_chunk(&mut iter, Some(root_key.as_slice())).unwrap();
        let ops = chunk.into_iter().map(|op| Ok(op));
        let chunk = verify_leaf(
            ops,
            [
                10, 147, 175, 167, 145, 38, 181, 73, 116, 253, 95, 138, 110, 222, 254, 197, 189,
                68, 11, 151,
            ],
        )
        .unwrap();
        let counts = count_node_types(chunk);
        assert_eq!(counts.kv, 15);
        assert_eq!(counts.hash, 0);
        assert_eq!(counts.kvhash, 0);

        // skip root node because that was our end key
        iter.next();

        // right leaf
        let chunk = get_next_chunk(&mut iter, None).unwrap();
        let ops = chunk.into_iter().map(|op| Ok(op));
        let chunk = verify_leaf(
            ops,
            [
                128, 166, 214, 176, 167, 251, 11, 84, 228, 2, 97, 239, 253, 75, 184, 16, 137, 134,
                72, 154,
            ],
        )
        .unwrap();
        let counts = count_node_types(chunk);
        assert_eq!(counts.kv, 15);
        assert_eq!(counts.hash, 0);
        assert_eq!(counts.kvhash, 0);
    }
}
