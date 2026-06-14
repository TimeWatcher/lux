use std::fmt;

pub const LUA_LOCAL_SLOT_LIMIT: usize = 200;

const WRAPPED_CHUNK_EXTRA_PARAMS: usize = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaLocalBudget {
    pub functions: Vec<LuaFunctionBudget>,
    pub max_slots: usize,
}

impl LuaLocalBudget {
    pub fn validate(&self) -> Result<(), LuaLocalBudgetError> {
        let offenders = self
            .functions
            .iter()
            .filter(|function| function.max_slots > LUA_LOCAL_SLOT_LIMIT)
            .cloned()
            .collect::<Vec<_>>();
        if offenders.is_empty() {
            Ok(())
        } else {
            Err(LuaLocalBudgetError { offenders })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaFunctionBudget {
    pub kind: LuaFunctionKind,
    pub start_line: usize,
    pub max_slots: usize,
    pub max_slots_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LuaFunctionKind {
    Chunk,
    Function,
}

impl LuaFunctionKind {
    fn label(&self) -> &'static str {
        match self {
            Self::Chunk => "chunk",
            Self::Function => "function",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaLocalBudgetError {
    pub offenders: Vec<LuaFunctionBudget>,
}

impl fmt::Display for LuaLocalBudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, offender) in self.offenders.iter().enumerate() {
            if index > 0 {
                writeln!(f)?;
            }
            write!(
                f,
                "generated Lua {} starting at line {} uses {} local slots at line {} (Lua 5.1 limit is {})",
                offender.kind.label(),
                offender.start_line,
                offender.max_slots,
                offender.max_slots_line,
                LUA_LOCAL_SLOT_LIMIT
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for LuaLocalBudgetError {}

pub fn analyze_lua_local_budget(lua: &str) -> LuaLocalBudget {
    BudgetParser::new(tokenize(lua)).analyze()
}

#[derive(Debug, Clone)]
struct FunctionFrame {
    kind: LuaFunctionKind,
    start_line: usize,
    scopes: Vec<usize>,
    active_slots: usize,
    max_slots: usize,
    max_slots_line: usize,
}

impl FunctionFrame {
    fn new(kind: LuaFunctionKind, start_line: usize, params: usize) -> Self {
        Self {
            kind,
            start_line,
            scopes: vec![params],
            active_slots: params,
            max_slots: params,
            max_slots_line: start_line,
        }
    }

    fn push_scope(&mut self, line: usize) {
        self.scopes.push(0);
        self.note_max(line);
    }

    fn pop_scope(&mut self) {
        if self.scopes.len() <= 1 {
            return;
        }
        if let Some(slots) = self.scopes.pop() {
            self.active_slots = self.active_slots.saturating_sub(slots);
        }
    }

    fn declare(&mut self, slots: usize, line: usize) {
        if slots == 0 {
            return;
        }
        if let Some(scope) = self.scopes.last_mut() {
            *scope += slots;
        }
        self.active_slots += slots;
        self.note_max(line);
    }

    fn note_max(&mut self, line: usize) {
        if self.active_slots > self.max_slots {
            self.max_slots = self.active_slots;
            self.max_slots_line = line;
        }
    }

    fn budget(self) -> LuaFunctionBudget {
        LuaFunctionBudget {
            kind: self.kind,
            start_line: self.start_line,
            max_slots: self.max_slots,
            max_slots_line: self.max_slots_line,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Function,
    Scope,
    Repeat,
}

struct BudgetParser {
    tokens: Vec<Token>,
    index: usize,
    functions: Vec<FunctionFrame>,
    blocks: Vec<BlockKind>,
    pending_for_slots: Option<usize>,
    finished: Vec<LuaFunctionBudget>,
}

impl BudgetParser {
    fn new(tokens: Vec<Token>) -> Self {
        let chunk = FunctionFrame::new(LuaFunctionKind::Chunk, 1, WRAPPED_CHUNK_EXTRA_PARAMS);
        Self {
            tokens,
            index: 0,
            functions: vec![chunk],
            blocks: Vec::new(),
            pending_for_slots: None,
            finished: Vec::new(),
        }
    }

    fn analyze(mut self) -> LuaLocalBudget {
        while !self.is_eof() {
            let token = self.current().clone();
            match token.kind {
                TokenKind::Keyword(Keyword::Local) => self.parse_local(token.line),
                TokenKind::Keyword(Keyword::Function) => {
                    self.parse_function(token.line, self.index, false);
                }
                TokenKind::Keyword(Keyword::For) => {
                    self.pending_for_slots = Some(self.count_for_names(self.index));
                }
                TokenKind::Keyword(Keyword::Do) | TokenKind::Keyword(Keyword::Then) => {
                    self.push_scope(token.line);
                }
                TokenKind::Keyword(Keyword::Repeat) => {
                    self.push_repeat_scope(token.line);
                }
                TokenKind::Keyword(Keyword::Else) => {
                    self.replace_branch_scope(token.line);
                }
                TokenKind::Keyword(Keyword::ElseIf) => {
                    self.pop_scope_block();
                }
                TokenKind::Keyword(Keyword::Until) => {
                    self.pop_until_repeat();
                }
                TokenKind::Keyword(Keyword::End) => {
                    self.close_block();
                }
                _ => {}
            }
            self.index += 1;
        }

        while self.functions.len() > 1 {
            let frame = self.functions.pop().expect("nested function frame");
            self.finished.push(frame.budget());
        }
        if let Some(chunk) = self.functions.pop() {
            self.finished.push(chunk.budget());
        }
        let max_slots = self
            .finished
            .iter()
            .map(|function| function.max_slots)
            .max()
            .unwrap_or(0);
        LuaLocalBudget {
            functions: self.finished,
            max_slots,
        }
    }

    fn current(&self) -> &Token {
        self.tokens
            .get(self.index)
            .unwrap_or_else(|| self.tokens.last().expect("token stream has eof"))
    }

    fn is_eof(&self) -> bool {
        matches!(self.current().kind, TokenKind::Eof)
    }

    fn current_function_mut(&mut self) -> Option<&mut FunctionFrame> {
        self.functions.last_mut()
    }

    fn push_scope(&mut self, line: usize) {
        self.blocks.push(BlockKind::Scope);
        let pending_for_slots = self.pending_for_slots.take();
        if let Some(function) = self.current_function_mut() {
            function.push_scope(line);
            if let Some(slots) = pending_for_slots {
                function.declare(slots, line);
            }
        }
    }

    fn push_repeat_scope(&mut self, line: usize) {
        self.blocks.push(BlockKind::Repeat);
        if let Some(function) = self.current_function_mut() {
            function.push_scope(line);
        }
    }

    fn replace_branch_scope(&mut self, line: usize) {
        self.pop_scope_block();
        self.push_scope(line);
    }

    fn pop_scope_block(&mut self) {
        if matches!(
            self.blocks.last(),
            Some(BlockKind::Scope | BlockKind::Repeat)
        ) {
            self.blocks.pop();
            if let Some(function) = self.current_function_mut() {
                function.pop_scope();
            }
        }
    }

    fn pop_until_repeat(&mut self) {
        if matches!(self.blocks.last(), Some(BlockKind::Repeat)) {
            self.blocks.pop();
            if let Some(function) = self.current_function_mut() {
                function.pop_scope();
            }
        }
    }

    fn close_block(&mut self) {
        match self.blocks.pop() {
            Some(BlockKind::Function) => {
                if let Some(frame) = self.functions.pop() {
                    self.finished.push(frame.budget());
                }
            }
            Some(BlockKind::Scope | BlockKind::Repeat) => {
                if let Some(function) = self.current_function_mut() {
                    function.pop_scope();
                }
            }
            None => {}
        }
    }

    fn parse_local(&mut self, line: usize) {
        let next_index = self.next_significant(self.index + 1);
        if self.keyword_at(next_index, Keyword::Function) {
            let name_index = self.next_significant(next_index + 1);
            if self.is_identifier(name_index) {
                if let Some(function) = self.current_function_mut() {
                    function.declare(1, line);
                }
            }
            self.parse_function(line, next_index, false);
            return;
        }

        let names = self.count_local_names(self.index + 1);
        if let Some(function) = self.current_function_mut() {
            function.declare(names, line);
        }
    }

    fn parse_function(&mut self, line: usize, function_index: usize, local_function: bool) {
        let (params, end_index) = self.parse_function_header(function_index, local_function);
        self.functions
            .push(FunctionFrame::new(LuaFunctionKind::Function, line, params));
        self.blocks.push(BlockKind::Function);
        self.index = end_index;
    }

    fn parse_function_header(&self, function_index: usize, local_function: bool) -> (usize, usize) {
        let mut index = function_index + 1;
        let mut implicit_self = false;
        while index < self.tokens.len() {
            match self.tokens[index].kind {
                TokenKind::Symbol('(') => break,
                TokenKind::Symbol(':') if !local_function => implicit_self = true,
                TokenKind::Eof => return (usize::from(implicit_self), function_index),
                _ => {}
            }
            index += 1;
        }

        if index >= self.tokens.len() || !matches!(self.tokens[index].kind, TokenKind::Symbol('('))
        {
            return (usize::from(implicit_self), function_index);
        }

        let mut params = usize::from(implicit_self);
        let mut depth = 0usize;
        while index < self.tokens.len() {
            match &self.tokens[index].kind {
                TokenKind::Symbol('(') => depth += 1,
                TokenKind::Symbol(')') => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return (params, index);
                    }
                }
                TokenKind::Ident(_) if depth == 1 => params += 1,
                TokenKind::Eof => break,
                _ => {}
            }
            index += 1;
        }

        (params, function_index)
    }

    fn count_local_names(&self, start: usize) -> usize {
        let mut count = 0usize;
        let mut index = start;
        let mut expecting_name = true;
        while index < self.tokens.len() {
            match &self.tokens[index].kind {
                TokenKind::Ident(_) if expecting_name => {
                    count += 1;
                    expecting_name = false;
                }
                TokenKind::Symbol(',') => expecting_name = true,
                TokenKind::Symbol('=') | TokenKind::Newline | TokenKind::Eof => break,
                _ => {}
            }
            index += 1;
        }
        count
    }

    fn count_for_names(&self, for_index: usize) -> usize {
        let mut count = 0usize;
        let mut index = for_index + 1;
        let mut expecting_name = true;
        while index < self.tokens.len() {
            match &self.tokens[index].kind {
                TokenKind::Ident(_) if expecting_name => {
                    count += 1;
                    expecting_name = false;
                }
                TokenKind::Symbol(',') => expecting_name = true,
                TokenKind::Symbol('=') | TokenKind::Keyword(Keyword::In) => break,
                TokenKind::Newline | TokenKind::Eof => break,
                _ => {}
            }
            index += 1;
        }
        count
    }

    fn next_significant(&self, mut index: usize) -> usize {
        while matches!(
            self.tokens.get(index).map(|token| &token.kind),
            Some(TokenKind::Newline)
        ) {
            index += 1;
        }
        index
    }

    fn keyword_at(&self, index: usize, keyword: Keyword) -> bool {
        matches!(
            self.tokens.get(index).map(|token| &token.kind),
            Some(TokenKind::Keyword(found)) if *found == keyword
        )
    }

    fn is_identifier(&self, index: usize) -> bool {
        matches!(
            self.tokens.get(index).map(|token| &token.kind),
            Some(TokenKind::Ident(_))
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    kind: TokenKind,
    line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TokenKind {
    Ident(String),
    Keyword(Keyword),
    Symbol(char),
    Newline,
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Keyword {
    Do,
    Else,
    ElseIf,
    End,
    For,
    Function,
    In,
    Local,
    Repeat,
    Then,
    Until,
}

fn tokenize(input: &str) -> Vec<Token> {
    let bytes = input.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0usize;
    let mut line = 1usize;

    while index < bytes.len() {
        let byte = bytes[index];
        match byte {
            b'\r' => {
                index += 1;
            }
            b'\n' => {
                tokens.push(Token {
                    kind: TokenKind::Newline,
                    line,
                });
                line += 1;
                index += 1;
            }
            b' ' | b'\t' | 0x0c => {
                index += 1;
            }
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                index += 2;
                if let Some((next, newlines)) = skip_long_bracket(bytes, index) {
                    line += newlines;
                    index = next;
                } else {
                    while index < bytes.len() && bytes[index] != b'\n' {
                        index += 1;
                    }
                }
            }
            b'\'' | b'"' => {
                let quote = byte;
                index += 1;
                while index < bytes.len() {
                    match bytes[index] {
                        b'\\' => {
                            index = (index + 2).min(bytes.len());
                        }
                        b'\n' => {
                            line += 1;
                            index += 1;
                        }
                        value if value == quote => {
                            index += 1;
                            break;
                        }
                        _ => index += 1,
                    }
                }
            }
            b'[' => {
                if let Some((next, newlines)) = skip_long_bracket(bytes, index) {
                    line += newlines;
                    index = next;
                } else {
                    tokens.push(Token {
                        kind: TokenKind::Symbol('['),
                        line,
                    });
                    index += 1;
                }
            }
            value if is_ident_start(value) => {
                let start = index;
                index += 1;
                while index < bytes.len() && is_ident_continue(bytes[index]) {
                    index += 1;
                }
                let text = &input[start..index];
                let kind = match keyword(text) {
                    Some(keyword) => TokenKind::Keyword(keyword),
                    None => TokenKind::Ident(text.to_string()),
                };
                tokens.push(Token { kind, line });
            }
            value => {
                tokens.push(Token {
                    kind: TokenKind::Symbol(value as char),
                    line,
                });
                index += 1;
            }
        }
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        line,
    });
    tokens
}

fn skip_long_bracket(bytes: &[u8], index: usize) -> Option<(usize, usize)> {
    if bytes.get(index) != Some(&b'[') {
        return None;
    }
    let mut cursor = index + 1;
    while bytes.get(cursor) == Some(&b'=') {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'[') {
        return None;
    }

    let equals = cursor - index - 1;
    cursor += 1;
    let mut newlines = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'\n' {
            newlines += 1;
            cursor += 1;
            continue;
        }
        if bytes[cursor] == b']' {
            let mut probe = cursor + 1;
            let mut found_equals = 0usize;
            while bytes.get(probe) == Some(&b'=') {
                found_equals += 1;
                probe += 1;
            }
            if found_equals == equals && bytes.get(probe) == Some(&b']') {
                return Some((probe + 1, newlines));
            }
        }
        cursor += 1;
    }

    Some((bytes.len(), newlines))
}

fn is_ident_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn is_ident_continue(byte: u8) -> bool {
    is_ident_start(byte) || byte.is_ascii_digit()
}

fn keyword(text: &str) -> Option<Keyword> {
    match text {
        "do" => Some(Keyword::Do),
        "else" => Some(Keyword::Else),
        "elseif" => Some(Keyword::ElseIf),
        "end" => Some(Keyword::End),
        "for" => Some(Keyword::For),
        "function" => Some(Keyword::Function),
        "in" => Some(Keyword::In),
        "local" => Some(Keyword::Local),
        "repeat" => Some(Keyword::Repeat),
        "then" => Some(Keyword::Then),
        "until" => Some(Keyword::Until),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{LUA_LOCAL_SLOT_LIMIT, LuaFunctionKind, analyze_lua_local_budget};

    fn repeated_locals(count: usize) -> String {
        (0..count)
            .map(|index| format!("local value{index} = {index}\n"))
            .collect()
    }

    #[test]
    fn chunk_budget_includes_gmod_wrapper_import_param() {
        let lua = repeated_locals(LUA_LOCAL_SLOT_LIMIT);
        let budget = analyze_lua_local_budget(&lua);
        let offender = budget
            .functions
            .iter()
            .find(|function| matches!(function.kind, LuaFunctionKind::Chunk))
            .expect("chunk budget");
        assert_eq!(offender.max_slots, LUA_LOCAL_SLOT_LIMIT + 1);
        assert!(budget.validate().is_err());
    }

    #[test]
    fn scoped_blocks_reduce_active_local_pressure() {
        let lua = format!(
            "do\n{}end\ndo\n{}end\n",
            repeated_locals(160),
            repeated_locals(160)
        );
        let budget = analyze_lua_local_budget(&lua);
        assert!(budget.validate().is_ok(), "{budget:#?}");
    }

    #[test]
    fn nested_functions_are_budgeted_independently() {
        let lua = format!(
            "local outer = 1\nlocal f = function(a, b)\n{}end\nlocal after = 2\n",
            repeated_locals(40)
        );
        let budget = analyze_lua_local_budget(&lua);
        let function = budget
            .functions
            .iter()
            .find(|function| matches!(function.kind, LuaFunctionKind::Function))
            .expect("nested function");
        assert_eq!(function.max_slots, 42);
        let chunk = budget
            .functions
            .iter()
            .find(|function| matches!(function.kind, LuaFunctionKind::Chunk))
            .expect("chunk");
        assert_eq!(chunk.max_slots, 4);
    }

    #[test]
    fn for_loop_names_are_scoped_to_loop_body() {
        let lua = format!(
            "for key, value in pairs(items) do\n{}end\nlocal done = true\n",
            repeated_locals(20)
        );
        let budget = analyze_lua_local_budget(&lua);
        let chunk = budget
            .functions
            .iter()
            .find(|function| matches!(function.kind, LuaFunctionKind::Chunk))
            .expect("chunk");
        assert_eq!(chunk.max_slots, 23);
    }

    #[test]
    fn strings_and_comments_do_not_create_fake_scopes() {
        let lua = r#"
local text = "function() local nope end"
-- function fake() local nope end
local block = [[
function fake()
  local nope
end
]]
function real(self, x)
  local ok = x
end
"#;
        let budget = analyze_lua_local_budget(lua);
        let function_count = budget
            .functions
            .iter()
            .filter(|function| matches!(function.kind, LuaFunctionKind::Function))
            .count();
        assert_eq!(function_count, 1);
        assert!(budget.validate().is_ok(), "{budget:#?}");
    }
}
