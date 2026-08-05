use super::{NodeKind, SyntaxNode, SyntaxToken, TokenKind};

pub(crate) trait TypedSyntaxNode<'tree>: Sized {
    fn cast(node: SyntaxNode<'tree>) -> Option<Self>;
    fn syntax(&self) -> &SyntaxNode<'tree>;
}

macro_rules! typed_node {
    ($name:ident, $kind:ident) => {
        #[allow(dead_code)]
        #[derive(Clone, Debug)]
        pub(crate) struct $name<'tree>(SyntaxNode<'tree>);

        impl<'tree> TypedSyntaxNode<'tree> for $name<'tree> {
            fn cast(node: SyntaxNode<'tree>) -> Option<Self> {
                (node.kind() == NodeKind::$kind).then_some(Self(node))
            }

            fn syntax(&self) -> &SyntaxNode<'tree> {
                &self.0
            }
        }
    };
}

typed_node!(FileSyntax, File);
typed_node!(StructDeclarationSyntax, StructDeclaration);
typed_node!(FunctionDeclarationSyntax, FunctionDeclaration);
typed_node!(FieldSyntax, Field);
typed_node!(BlockExpressionSyntax, BlockExpression);
typed_node!(PostfixExpressionSyntax, PostfixExpression);
typed_node!(CallSuffixSyntax, CallSuffix);

#[derive(Clone, Debug)]
pub(crate) enum DeclarationSyntax<'tree> {
    Struct(StructDeclarationSyntax<'tree>),
    Function(FunctionDeclarationSyntax<'tree>),
    Unsupported(SyntaxNode<'tree>),
}

impl<'tree> FileSyntax<'tree> {
    pub(crate) fn declarations(&self) -> Vec<DeclarationSyntax<'tree>> {
        self.syntax()
            .child_nodes()
            .into_iter()
            .filter_map(|node| match node.kind() {
                NodeKind::StructDeclaration => {
                    StructDeclarationSyntax::cast(node).map(DeclarationSyntax::Struct)
                }
                NodeKind::FunctionDeclaration => {
                    FunctionDeclarationSyntax::cast(node).map(DeclarationSyntax::Function)
                }
                NodeKind::ImportDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::InterfaceDeclaration => Some(DeclarationSyntax::Unsupported(node)),
                _ => None,
            })
            .collect()
    }
}

impl<'tree> StructDeclarationSyntax<'tree> {
    pub(crate) fn name_token(&self) -> Option<SyntaxToken<'tree>> {
        token_after(self.syntax(), TokenKind::Struct, TokenKind::Identifier)
    }

    pub(crate) fn fields(&self) -> Vec<FieldSyntax<'tree>> {
        children_of_type(self.syntax())
    }
}

impl<'tree> FieldSyntax<'tree> {
    pub(crate) fn name_token(&self) -> Option<SyntaxToken<'tree>> {
        self.syntax()
            .child_tokens()
            .into_iter()
            .find(|token| token.kind() == TokenKind::Identifier && !token.is_missing())
    }
}

impl<'tree> FunctionDeclarationSyntax<'tree> {
    pub(crate) fn is_public(&self) -> bool {
        self.syntax()
            .child_tokens()
            .iter()
            .any(|token| token.kind() == TokenKind::Pub)
    }

    pub(crate) fn name_token(&self) -> Option<SyntaxToken<'tree>> {
        token_after(self.syntax(), TokenKind::Fn, TokenKind::Identifier)
    }

    pub(crate) fn body(&self) -> Option<BlockExpressionSyntax<'tree>> {
        self.syntax()
            .child_nodes()
            .into_iter()
            .find_map(BlockExpressionSyntax::cast)
    }
}

impl<'tree> PostfixExpressionSyntax<'tree> {
    #[allow(dead_code)]
    pub(crate) fn call_suffixes(&self) -> Vec<CallSuffixSyntax<'tree>> {
        children_of_type(self.syntax())
    }
}

fn children_of_type<'tree, T: TypedSyntaxNode<'tree>>(node: &SyntaxNode<'tree>) -> Vec<T> {
    node.child_nodes().into_iter().filter_map(T::cast).collect()
}

fn token_after<'tree>(
    node: &SyntaxNode<'tree>,
    preceding: TokenKind,
    wanted: TokenKind,
) -> Option<SyntaxToken<'tree>> {
    let mut saw_preceding = false;
    node.child_tokens().into_iter().find(|token| {
        if token.kind() == preceding {
            saw_preceding = true;
            return false;
        }
        saw_preceding && token.kind() == wanted && !token.is_missing()
    })
}
