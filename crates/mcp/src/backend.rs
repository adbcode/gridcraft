use serde_json::Value;

/// A headless in-process engine that answers MCP method calls.
pub trait Backend {
    /// Call one method. `Ok` carries the result and `Err` carries a readable error message.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String>;

    /// Short backend description shown when the MCP server starts.
    fn describe(&self) -> String;
}
