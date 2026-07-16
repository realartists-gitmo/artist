use rig_core::tool::Tool;
use serde::Deserialize;
use serde_json::{Value, json};
use std::convert::Infallible;

/// Adds two signed integers.
#[derive(Clone, Copy)]
pub(crate) struct Add;

#[derive(Deserialize)]
pub(crate) struct AddArgs {
    left: i64,
    right: i64,
}

impl Tool for Add {
    const NAME: &'static str = "add_integers";
    type Error = Infallible;
    type Args = AddArgs;
    type Output = i64;

    fn description(&self) -> String {
        "Add two signed integers and return their sum.".into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "left": { "type": "integer" },
                "right": { "type": "integer" }
            },
            "required": ["left", "right"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: AddArgs) -> Result<i64, Infallible> {
        Ok(args.left + args.right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_signed_integers() {
        let result = futures::executor::block_on(Add.call(AddArgs { left: -2, right: 5 }));
        assert_eq!(result, Ok(3));
    }
}
