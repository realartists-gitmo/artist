use rig_core::tool::Tool;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{error::Error, fmt};

/// Adds two signed integers.
#[derive(Clone, Copy)]
pub(crate) struct Add;

#[derive(Deserialize)]
pub(crate) struct AddArgs {
    left: i64,
    right: i64,
}

#[derive(Debug)]
pub(crate) struct AddError;

impl fmt::Display for AddError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("integer addition overflowed")
    }
}
impl Error for AddError {}

impl Tool for Add {
    const NAME: &'static str = "add_integers";
    type Error = AddError;
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

    async fn call(&self, args: AddArgs) -> Result<i64, AddError> {
        args.left.checked_add(args.right).ok_or(AddError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_signed_integers() {
        let result = futures::executor::block_on(Add.call(AddArgs { left: -2, right: 5 }));
        assert_eq!(result.unwrap(), 3);
        let overflow = futures::executor::block_on(Add.call(AddArgs {
            left: i64::MAX,
            right: 1,
        }));
        assert!(overflow.is_err());
    }
}
