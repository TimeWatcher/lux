mod lua;
mod lua_budget;

pub use lua::{CodegenError, LuaCodegen, LuaOutput};
pub use lua_budget::{
    LUA_LOCAL_SLOT_LIMIT, LuaFunctionBudget, LuaFunctionKind, LuaLocalBudget, LuaLocalBudgetError,
    analyze_lua_local_budget,
};
