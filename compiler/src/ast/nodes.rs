use crate::source::SourceSpan;

#[derive(Debug, Clone, PartialEq)]
pub struct Identifier {
    pub name: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub body: Vec<Stmt>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub statements: Vec<Stmt>,
    pub tail: Option<Expr>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    LocalDecl {
        mode: BindingMode,
        names: Vec<Identifier>,
        values: Vec<Expr>,
    },
    LocalDestructure {
        mode: BindingMode,
        patterns: Vec<Pattern>,
        values: Vec<Expr>,
    },
    Assign {
        targets: Vec<Expr>,
        values: Vec<Expr>,
    },
    CompoundAssign {
        target: Expr,
        op: CompoundAssignOp,
        value: Expr,
    },
    Expr(Expr),
    Return(Vec<Expr>),
    Break,
    Continue,
    Import(ImportStmt),
    PartOrderDecl(PartOrderDecl),
    ExternDecl(ExternDecl),
    HostPackageDecl(HostPackageDecl),
    ExportDecl {
        kind: ExportKind,
        realm: Option<Realm>,
        stmt: Box<Stmt>,
    },
    ExportList {
        realm: Option<Realm>,
        entries: Vec<ExportSpecifier>,
    },
    ExportAll {
        realm: Option<Realm>,
    },
    RealmDecl {
        realm: Realm,
        stmt: Box<Stmt>,
    },
    RealmBlock {
        realm: Realm,
        block: Block,
    },
    InitDecl {
        realm: Option<Realm>,
        block: Block,
    },
    FunctionDecl(FunctionDecl),
    EnumDecl(EnumDecl),
    If {
        condition: Expr,
        then_block: Block,
        else_block: Option<Block>,
    },
    While {
        condition: Expr,
        body: Block,
    },
    NumericFor {
        name: Identifier,
        start: Expr,
        end: Expr,
        step: Option<Expr>,
        body: Block,
    },
    GenericFor {
        names: Vec<Identifier>,
        iter: Vec<Expr>,
        body: Block,
    },
    RepeatUntil {
        body: Block,
        condition: Expr,
    },
    Do(Block),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumDecl {
    pub name: Identifier,
    pub repr: EnumRepr,
    pub runtime: bool,
    pub variants: Vec<EnumVariant>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EnumRepr {
    String,
    Number,
    Table { tag_field: String },
    Existing { tag_field: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariant {
    pub name: Identifier,
    pub payload: EnumVariantPayload,
    pub tag: Option<Expr>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EnumVariantPayload {
    None,
    Tuple(Vec<Identifier>),
    Record(Vec<Identifier>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Realm {
    Shared,
    Client,
    Server,
}

impl Realm {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Client => "client",
            Self::Server => "server",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "shared" => Some(Self::Shared),
            "client" => Some(Self::Client),
            "server" => Some(Self::Server),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingMode {
    Local,
    Const,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportStmt {
    pub source: String,
    pub specifiers: Vec<ImportSpecifier>,
    pub side_effect_only: bool,
    pub phase: ImportPhase,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImportSpecifier {
    Named {
        imported: Identifier,
        local: Identifier,
    },
    Namespace {
        local: Identifier,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportPhase {
    Runtime,
    Macro,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    Runtime,
    Macro,
    HostExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportSpecifier {
    pub exported: Identifier,
    pub local: Identifier,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExternDecl {
    pub realm: Realm,
    pub path: Vec<Identifier>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartOrderDecl {
    pub kind: PartOrderKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartOrderRelation {
    Before,
    After,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PartOrderKind {
    Relative {
        relation: PartOrderRelation,
        target: String,
    },
    Order {
        targets: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct HostPackageDecl {
    pub target: String,
    pub runtime: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDecl {
    pub name: FunctionName,
    pub params: Vec<Param>,
    pub vararg: bool,
    pub body: FunctionBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionName {
    Simple(Identifier),
    Dotted(Vec<Identifier>),
    Method {
        receiver: Vec<Identifier>,
        method: Identifier,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionBody {
    Expr(Box<Expr>),
    Block(Box<Block>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: Identifier,
    pub default: Option<Expr>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternKind {
    Identifier(Identifier),
    Object(Vec<ObjectPatternField>),
    Array(Vec<ArrayPatternItem>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectPatternField {
    pub key: Identifier,
    pub pattern: Pattern,
    pub default: Option<Expr>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArrayPatternItem {
    pub pattern: Pattern,
    pub default: Option<Expr>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompoundAssignOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Identifier(Identifier),
    Nil,
    Boolean(bool),
    Number(String),
    String(String),
    Vararg,
    PipelinePlaceholder,
    TemplateString(Vec<TemplatePart>),
    Table(TableExpr),
    Paren(Box<Expr>),
    Unary {
        op: UnaryOp,
        argument: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Conditional {
        condition: Box<Expr>,
        then_branch: ExprOrBlock,
        else_branch: ExprOrBlock,
        form: ConditionalForm,
    },
    Match(MatchExpr),
    Do(Box<Block>),
    Function(FunctionExpr),
    Chain(ChainExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchExpr {
    pub subject: Box<Expr>,
    pub arms: Vec<MatchArm>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: MatchPattern,
    pub body: ExprOrBlock,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchPattern {
    pub kind: MatchPatternKind,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchPatternKind {
    Or(Vec<MatchPattern>),
    Wildcard,
    Binding(Identifier),
    Literal(MatchLiteral),
    Variant {
        path: Vec<Identifier>,
        payload: Option<MatchPatternPayload>,
    },
    Object(Vec<MatchObjectPatternField>),
    Array(Vec<MatchArrayPatternItem>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchLiteral {
    Nil,
    Boolean(bool),
    Number(String),
    String(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchPatternPayload {
    Tuple(Vec<MatchPattern>),
    Record(Vec<MatchObjectPatternField>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchObjectPatternField {
    pub key: Identifier,
    pub pattern: MatchPattern,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArrayPatternItem {
    pub pattern: MatchPattern,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprOrBlock {
    Expr(Box<Expr>),
    Block(Box<Block>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionalForm {
    IfExpr,
    ThenElse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionExpr {
    pub params: Vec<Param>,
    pub vararg: bool,
    pub param_span: SourceSpan,
    pub body: FunctionBody,
    pub arrow_kind: ArrowKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowKind {
    Normal,
    ImplicitSelf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Len,
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
    Coalesce,
    Pipe,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TemplatePart {
    pub kind: TemplatePartKind,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TemplatePartKind {
    Text(String),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableExpr {
    pub fields: Vec<TableField>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableField {
    pub kind: TableFieldKind,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TableFieldKind {
    Array(Expr),
    Named { name: Identifier, value: Expr },
    ExprKey { key: Expr, value: Expr },
    Spread(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChainExpr {
    pub base: Box<Expr>,
    pub segments: Vec<ChainSegment>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChainSegment {
    pub kind: ChainSegmentKind,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChainSegmentKind {
    Member {
        name: Identifier,
        optional: bool,
    },
    Index {
        index: Expr,
        optional: bool,
    },
    Call {
        args: Vec<Expr>,
        style: CallStyle,
    },
    SafeDotCall {
        name: Identifier,
        args: Vec<Expr>,
        style: CallStyle,
    },
    MethodCall {
        name: Identifier,
        args: Vec<Expr>,
        optional: bool,
        style: CallStyle,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallStyle {
    Paren,
    TailTable,
    TailString,
}
