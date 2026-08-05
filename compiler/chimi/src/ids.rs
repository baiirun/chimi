macro_rules! identifier {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub(crate) struct $name(u32);

        impl $name {
            pub(crate) const fn new(value: u32) -> Self {
                Self(value)
            }

            pub(crate) const fn raw(self) -> u32 {
                self.0
            }
        }
    };
}

identifier!(FileId);
identifier!(ModuleId);
identifier!(SyntaxId);
identifier!(SemanticId);
identifier!(SymbolId);
identifier!(FailureRowId);
identifier!(NeedsRowId);
identifier!(OriginId);
identifier!(CoreNodeId);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct TypeId(u32);

impl TypeId {
    pub(crate) const fn new(value: u32) -> Self {
        Self(value)
    }

    pub(crate) const fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextRange {
    start: u32,
    end: u32,
}

impl TextRange {
    pub(crate) fn new(start: usize, end: usize) -> Self {
        debug_assert!(start <= end);
        Self {
            start: u32::try_from(start).expect("source offsets fit in u32"),
            end: u32::try_from(end).expect("source offsets fit in u32"),
        }
    }

    pub(crate) fn empty(offset: usize) -> Self {
        Self::new(offset, offset)
    }

    pub const fn start(self) -> u32 {
        self.start
    }

    pub const fn end(self) -> u32 {
        self.end
    }
}
