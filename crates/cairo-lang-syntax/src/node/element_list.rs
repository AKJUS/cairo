use std::marker::PhantomData;
use std::sync::Arc;

use super::SyntaxGroup;
use crate::node::{SyntaxNode, TypedSyntaxNode};

// A typed view of an element list node.
// STEP=1 means a sequence of elements (e.g. sequence of trivia elements).
// STEP=2 means a separated sequence (e.g. argument list separated by `,`).
#[derive(Clone, Debug, Eq, Hash, PartialEq, salsa::Update)]
pub struct ElementList<'db, T: TypedSyntaxNode<'db>, const STEP: usize> {
    pub node: SyntaxNode<'db>,
    phantom: PhantomData<T>,
}
impl<'db, T: TypedSyntaxNode<'db>, const STEP: usize> ElementList<'db, T, STEP> {
    pub fn new(node: SyntaxNode<'db>) -> Self {
        Self { node, phantom: PhantomData {} }
    }
}
impl<'db, T: TypedSyntaxNode<'db>> ElementList<'db, T, 1> {
    pub fn elements_vec(&self, db: &'db dyn SyntaxGroup) -> Vec<T> {
        self.elements(db).collect()
    }
    pub fn elements<'a: 'db>(
        &self,
        db: &'db dyn SyntaxGroup,
    ) -> impl ExactSizeIterator<Item = T> + DoubleEndedIterator + 'db {
        ElementListRawIter::new(self.node.get_children(db).as_slice().into())
            .map(move |x| T::from_syntax_node(db, x))
    }
    pub fn has_tail(&self, _db: &dyn SyntaxGroup) -> bool {
        false
    }
}
impl<'db, T: TypedSyntaxNode<'db>> ElementList<'db, T, 2> {
    pub fn elements_vec(&self, db: &'db dyn SyntaxGroup) -> Vec<T> {
        self.elements(db).collect()
    }
    pub fn elements(
        &self,
        db: &'db dyn SyntaxGroup,
    ) -> impl ExactSizeIterator<Item = T> + DoubleEndedIterator + 'db {
        ElementListRawIter::new(self.node.get_children(db).as_slice().into())
            .step_by(2)
            .map(move |x| T::from_syntax_node(db, x))
    }
    pub fn has_tail(&self, db: &dyn SyntaxGroup) -> bool {
        !self.node.get_children(db).len().is_multiple_of(2)
    }
}

/// Iterator over the raw elements of an `ElementList`.
struct ElementListRawIter<'a> {
    /// The `Arc` storing the actual node.
    _data: Arc<[SyntaxNode<'a>]>,
    /// Actual iterator over the elements.
    iter: std::slice::Iter<'a, SyntaxNode<'a>>,
}

impl<'a> ElementListRawIter<'a> {
    fn new(data: Arc<[SyntaxNode<'a>]>) -> Self {
        // We leak the Arc to get a 'static reference, and keep the Arc in the struct to avoid
        // leaks.
        let ptr: *const [SyntaxNode<'_>] = Arc::as_ptr(&data);
        let slice: &'static [SyntaxNode<'_>] = unsafe { std::mem::transmute(&*ptr) };
        let iter = slice.iter();
        Self { _data: data, iter }
    }
}

impl<'a> Iterator for ElementListRawIter<'a> {
    type Item = SyntaxNode<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().copied()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}
impl ExactSizeIterator for ElementListRawIter<'_> {
    fn len(&self) -> usize {
        self.iter.len()
    }
}
impl DoubleEndedIterator for ElementListRawIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.iter.next_back().copied()
    }
}
