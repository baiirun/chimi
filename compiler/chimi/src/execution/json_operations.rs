//! The native core of `std/json` on the reference interpreter. `runtime.c`
//! implements the same operations in `invoke_json_operation`, and the
//! `std-tests` programs run on both and must agree.

use super::{Control, Environment, Evaluator, RuntimeMap, RuntimeValue};
use crate::ids::CoreNodeId;
use crate::json::{
    JsonData, NumberConversionError, decode_json, number_to_decimal, number_to_int,
    write_json_data,
};
use crate::standard::{StandardConstructor, StandardFunction, StandardSymbol};

impl Evaluator<'_> {
    /// Requires: `function` is one of the `std/json` operations of the registry.
    pub(super) fn invoke_json_operation(
        &mut self,
        function: StandardFunction,
        arguments: Vec<RuntimeValue>,
        span: u64,
        call_node: CoreNodeId,
        environment: Environment,
    ) -> Control {
        let _ = (span, environment);
        match (function, arguments.as_slice()) {
            (StandardFunction::JsonDecode, [RuntimeValue::Text(source)]) => {
                match decode_json(source) {
                    Ok(value) => Control::Success(self.json_to_runtime(value)),
                    Err(error) => Control::failure(self.standard_value(
                        StandardConstructor::DecodeInvalidJson,
                        vec![
                            RuntimeValue::Text(error.message),
                            RuntimeValue::Integer(i64::try_from(error.offset).unwrap_or(i64::MAX)),
                        ],
                    )),
                }
            }
            (StandardFunction::JsonEncode, [value]) => match self.runtime_to_json(value) {
                Ok(value) => {
                    let mut output = String::new();
                    write_json_data(&mut output, &value);
                    Control::Success(RuntimeValue::Text(output))
                }
                Err(()) => self.intrinsic_shape_defect(function, call_node),
            },
            (StandardFunction::JsonInt, [RuntimeValue::Integer(value)]) => {
                Control::Success(self.standard_value(
                    StandardConstructor::JsonNumber,
                    vec![RuntimeValue::JsonNumber(value.to_string())],
                ))
            }
            (StandardFunction::JsonNumberToInt, [RuntimeValue::JsonNumber(number)]) => {
                match number_to_int(number) {
                    Ok(value) => Control::Success(RuntimeValue::Integer(value)),
                    Err(error) => {
                        let variant = match error {
                            NumberConversionError::NotInteger => StandardConstructor::JsonNotInteger,
                            NumberConversionError::OutOfRange => StandardConstructor::JsonOutOfRange,
                        };
                        Control::failure(self.standard_value(variant, Vec::new()))
                    }
                }
            }
            (StandardFunction::JsonNumberToDecimal, [RuntimeValue::JsonNumber(number)]) => {
                match number_to_decimal(number) {
                    Some(value) => Control::Success(RuntimeValue::Decimal(value)),
                    None => Control::failure(
                        self.standard_value(StandardConstructor::JsonOutOfRange, Vec::new()),
                    ),
                }
            }
            _ => self.intrinsic_shape_defect(function, call_node),
        }
    }

    fn json_to_runtime(&self, value: JsonData) -> RuntimeValue {
        match value {
            JsonData::Null => self.standard_value(StandardConstructor::JsonNull, Vec::new()),
            JsonData::Boolean(value) => self.standard_value(
                StandardConstructor::JsonBoolean,
                vec![RuntimeValue::Boolean(value)],
            ),
            JsonData::Number(value) => self.standard_value(
                StandardConstructor::JsonNumber,
                vec![RuntimeValue::JsonNumber(value)],
            ),
            JsonData::Text(value) => self.standard_value(
                StandardConstructor::JsonText,
                vec![RuntimeValue::Text(value)],
            ),
            JsonData::Array(values) => self.standard_value(
                StandardConstructor::JsonArray,
                vec![RuntimeValue::List(
                    values
                        .into_iter()
                        .map(|value| self.json_to_runtime(value))
                        .collect(),
                )],
            ),
            JsonData::Object(fields) => {
                let mut map = RuntimeMap::default();
                for (name, value) in fields {
                    map = map.put(
                        RuntimeValue::Text(name),
                        self.json_to_runtime(value),
                        &self.key_order(),
                    );
                }
                self.standard_value(
                    StandardConstructor::JsonObject,
                    vec![RuntimeValue::Map(map)],
                )
            }
        }
    }

    /// `Err` when `value` is not a well-formed `Json.Value`, which a verified
    /// program never builds.
    fn runtime_to_json(&self, value: &RuntimeValue) -> Result<JsonData, ()> {
        let RuntimeValue::Constructed {
            constructor,
            fields,
            ..
        } = value
        else {
            return Err(());
        };
        let Some(StandardSymbol::Constructor(constructor)) =
            self.core.symbol(*constructor).standard
        else {
            return Err(());
        };
        let field = |index: usize| fields.get(index).map(|field| &field.value).ok_or(());
        match constructor {
            StandardConstructor::JsonNull => Ok(JsonData::Null),
            StandardConstructor::JsonBoolean => match field(0)? {
                RuntimeValue::Boolean(value) => Ok(JsonData::Boolean(*value)),
                _ => Err(()),
            },
            StandardConstructor::JsonNumber => match field(0)? {
                RuntimeValue::JsonNumber(value) => Ok(JsonData::Number(value.clone())),
                _ => Err(()),
            },
            StandardConstructor::JsonText => match field(0)? {
                RuntimeValue::Text(value) => Ok(JsonData::Text(value.clone())),
                _ => Err(()),
            },
            StandardConstructor::JsonArray => match field(0)? {
                RuntimeValue::List(values) => values
                    .iter()
                    .map(|value| self.runtime_to_json(value))
                    .collect::<Result<Vec<_>, _>>()
                    .map(JsonData::Array),
                _ => Err(()),
            },
            StandardConstructor::JsonObject => match field(0)? {
                RuntimeValue::Map(fields) => fields
                    .entries()
                    .into_iter()
                    .map(|(name, value)| match &name {
                        RuntimeValue::Text(name) => self
                            .runtime_to_json(&value)
                            .map(|value| (name.clone(), value)),
                        _ => Err(()),
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(JsonData::Object),
                _ => Err(()),
            },
            _ => Err(()),
        }
    }
}
