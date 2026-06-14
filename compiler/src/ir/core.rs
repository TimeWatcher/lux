use crate::ast::{BinaryOp, BindingMode, CompoundAssignOp, FunctionName, UnaryOp};
use crate::resolve::{Export, ResolvedSymbol};
use crate::source::SourceSpan;

#[derive(Debug, Clone, PartialEq)]
pub struct IrModule {
    pub body: Vec<IrStmt>,
    pub exports: Vec<Export>,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrBlock {
    pub statements: Vec<IrStmt>,
    pub tail: Option<IrExpr>,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrStmt {
    pub kind: IrStmtKind,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrStmtKind {
    Noop,
    LocalDecl {
        mode: BindingMode,
        names: Vec<String>,
        values: Vec<IrExpr>,
    },
    LocalDestructure {
        mode: BindingMode,
        patterns: Vec<IrPattern>,
        values: Vec<IrExpr>,
    },
    Assign {
        targets: Vec<IrPlace>,
        values: Vec<IrExpr>,
    },
    CompoundAssign {
        target: IrPlace,
        op: CompoundAssignOp,
        value: IrExpr,
    },
    Expr(IrExpr),
    Return(Vec<IrExpr>),
    Break,
    Continue,
    FunctionDecl(IrFunctionDecl),
    EnumDecl(IrEnumDecl),
    If {
        condition: IrExpr,
        then_block: IrBlock,
        else_block: Option<IrBlock>,
    },
    While {
        condition: IrExpr,
        body: IrBlock,
    },
    NumericFor {
        name: String,
        start: IrExpr,
        end: IrExpr,
        step: Option<IrExpr>,
        body: IrBlock,
    },
    GenericFor {
        names: Vec<String>,
        iter: Vec<IrExpr>,
        body: IrBlock,
    },
    RepeatUntil {
        body: IrBlock,
        condition: IrExpr,
    },
    Do(IrBlock),
    Import {
        source: String,
        specifiers: Vec<IrImportSpecifier>,
        side_effect_only: bool,
    },
    ExportList(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrEnumDecl {
    pub name: String,
    pub repr: IrEnumRepr,
    pub runtime: bool,
    pub variants: Vec<IrEnumVariant>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrEnumRepr {
    String,
    Number,
    Table { tag_field: String },
    Existing { tag_field: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrEnumVariant {
    pub name: String,
    pub payload: IrEnumVariantPayload,
    pub tag: IrExpr,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrEnumVariantPayload {
    None,
    Tuple(Vec<String>),
    Record(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrImportSpecifier {
    pub imported: String,
    pub local: String,
    pub namespace: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrFunctionDecl {
    pub name: FunctionName,
    pub params: Vec<IrParam>,
    pub vararg: bool,
    pub body: IrFunctionBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrFunctionBody {
    Expr(Box<IrExpr>),
    Block(Box<IrBlock>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrExpr {
    pub kind: IrExprKind,
    pub origin: Origin,
    pub value_mode: ValueMode,
    pub symbol: Option<ResolvedSymbol>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrExprKind {
    Identifier(String),
    Nil,
    Boolean(bool),
    Number(String),
    String(String),
    Vararg,
    PipelinePlaceholder,
    Template(Vec<IrTemplatePart>),
    Table(Vec<IrTableField>),
    Unary {
        op: UnaryOp,
        argument: Box<IrExpr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<IrExpr>,
        right: Box<IrExpr>,
    },
    Conditional {
        condition: Box<IrExpr>,
        then_branch: IrExprOrBlock,
        else_branch: IrExprOrBlock,
    },
    Match(IrMatchExpr),
    Do(Box<IrBlock>),
    Function(IrFunctionExpr),
    Chain(IrChain),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrMatchExpr {
    pub subject: Box<IrExpr>,
    pub arms: Vec<IrMatchArm>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrMatchArm {
    pub pattern: IrMatchPattern,
    pub body: IrExprOrBlock,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrMatchPattern {
    pub kind: IrMatchPatternKind,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrMatchPatternKind {
    Or(Vec<IrMatchPattern>),
    Wildcard,
    Binding(String),
    Literal(IrMatchLiteral),
    Variant {
        path: Vec<String>,
        payload: Option<IrMatchPatternPayload>,
    },
    Object(Vec<IrMatchObjectPatternField>),
    Array(Vec<IrMatchArrayPatternItem>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrMatchLiteral {
    Nil,
    Boolean(bool),
    Number(String),
    String(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrMatchPatternPayload {
    Tuple(Vec<IrMatchPattern>),
    Record(Vec<IrMatchObjectPatternField>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrMatchObjectPatternField {
    pub key: String,
    pub pattern: IrMatchPattern,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrMatchArrayPatternItem {
    pub pattern: IrMatchPattern,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrExprOrBlock {
    Expr(Box<IrExpr>),
    Block(Box<IrBlock>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrFunctionExpr {
    pub params: Vec<IrParam>,
    pub vararg: bool,
    pub implicit_self: bool,
    pub body: IrFunctionBody,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrParam {
    pub name: String,
    pub default: Option<IrExpr>,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrPattern {
    pub kind: IrPatternKind,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrPatternKind {
    Identifier(String),
    Object(Vec<IrObjectPatternField>),
    Array(Vec<IrArrayPatternItem>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrObjectPatternField {
    pub key: String,
    pub pattern: IrPattern,
    pub default: Option<IrExpr>,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrArrayPatternItem {
    pub pattern: IrPattern,
    pub default: Option<IrExpr>,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrTemplatePart {
    pub kind: IrTemplatePartKind,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrTemplatePartKind {
    Text(String),
    Expr(IrExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrTableField {
    pub kind: IrTableFieldKind,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrTableFieldKind {
    Array(IrExpr),
    Named { name: String, value: IrExpr },
    ExprKey { key: IrExpr, value: IrExpr },
    Spread(IrExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrChain {
    pub base: Box<IrExpr>,
    pub segments: Vec<IrChainSegment>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrChainSegment {
    pub kind: IrChainSegmentKind,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrChainSegmentKind {
    Member {
        name: String,
        optional: bool,
    },
    Index {
        index: IrExpr,
        optional: bool,
    },
    Call {
        args: Vec<IrExpr>,
        style: IrCallStyle,
    },
    SafeDotCall {
        name: String,
        args: Vec<IrExpr>,
        style: IrCallStyle,
    },
    MethodCall {
        name: String,
        args: Vec<IrExpr>,
        optional: bool,
        style: IrCallStyle,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrCallStyle {
    Paren,
    TailTable,
    TailString,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrPlace {
    Identifier(String),
    Member { object: IrExpr, name: String },
    Index { object: IrExpr, index: IrExpr },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueMode {
    Single,
    MultiTail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Source(SourceSpan),
    Synthetic { source: SourceSpan, reason: String },
}

impl Origin {
    pub const fn source(span: SourceSpan) -> Self {
        Self::Source(span)
    }

    pub fn span(&self) -> SourceSpan {
        match self {
            Origin::Source(span) => *span,
            Origin::Synthetic { source, .. } => *source,
        }
    }
}
