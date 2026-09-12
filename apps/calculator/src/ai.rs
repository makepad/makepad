//! calculator.eval: a read-only evaluator over the root's current angle.

use crate::engine::{self, format_number, MAX_SOURCE_BYTES};
use crate::model::{AngleMode, CalculatorDoc, ErrorCode};
use makepad_ai_services::wire::{Risk, ServiceCall, ServiceManifest, ToolDef, ToolResult};
use makepad_strict_json::{self as json, Value};

pub fn manifest() -> ServiceManifest {
    ServiceManifest::new(
        "calculator",
        "Calculator",
        "Evaluate a mathematical expression in the calculator's current angle mode.",
    )
    .with_tool(ToolDef::new(
        "eval",
        "Evaluate an expression. Uses the on-screen angle mode. Does not change the calculation, memory, or history.",
        r#"{"type":"object","properties":{"expression":{"type":"string","maxLength":1024}},"required":["expression"],"additionalProperties":false}"#,
        Risk::Read,
    ))
}

pub fn answer(doc: &CalculatorDoc, call: &ServiceCall) -> ToolResult {
    match call.tool.as_str() {
        "eval" => eval_on(doc, call),
        other => ToolResult::refused(&call.call_id, format!("unknown tool `{other}`; this app only has `eval`")),
    }
}

pub fn eval_on(doc: &CalculatorDoc, call: &ServiceCall) -> ToolResult {
    let expression = match parse_args(&call.args) {
        Ok(s) => s,
        Err(msg) => return ToolResult::refused(&call.call_id, msg),
    };
    if expression.len() > MAX_SOURCE_BYTES {
        return ToolResult::refused(&call.call_id, "expression exceeds 1024 bytes");
    }
    match engine::evaluate(&expression, doc.angle) {
        Ok(value) => {
            let formatted = format_number(value);
            let data = json::obj(vec![
                ("value", json_num(value)),
                ("formatted", json::s(&formatted)),
                ("angle", json::s(doc.angle.as_str())),
            ])
            .to_json();
            ToolResult::ok(&call.call_id, formatted, "").with_data(data)
        }
        Err(e) if e.code == ErrorCode::Limit => ToolResult::refused(&call.call_id, "expression limit exceeded"),
        Err(e) => {
            let data = json::obj(vec![
                ("code", json::s(e.code.as_str())),
                ("byte_offset", Value::Int(e.byte_offset as i64)),
            ])
            .to_json();
            ToolResult::failed(
                &call.call_id,
                format!("{} at {}", e.code.as_str(), e.byte_offset),
            )
            .with_data(data)
        }
    }
}

fn json_num(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < i64::MAX as f64 && (v as i64) as f64 == v {
        Value::Int(v as i64)
    } else {
        Value::F64(v)
    }
}

fn parse_args(args: &str) -> Result<String, String> {
    if args.len() > 8192 { return Err("arguments too large".into()); }
    let value = json::parse(args.as_bytes()).map_err(|_| "invalid arguments".to_string())?;
    let Value::Obj(pairs) = value else {
        return Err("arguments must be an object".into());
    };
    let mut expression = None;
    for (k, v) in &pairs {
        match k.as_str() {
            "expression" => {
                let s = v.as_str().ok_or_else(|| "expression must be a string".to_string())?;
                if s.len() > 1024 {
                    return Err("expression exceeds maxLength 1024".into());
                }
                expression = Some(s.to_string());
            }
            other => return Err(format!("unexpected property `{other}`")),
        }
    }
    expression.ok_or_else(|| "missing expression".to_string())
}

/// Used by tests that do not construct a view.
pub fn eval_expression(expression: &str, angle: AngleMode) -> Result<(f64, String), ErrorCode> {
    engine::evaluate(expression, angle)
        .map(|v| (v, format_number(v)))
        .map_err(|e| e.code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seed;
    use makepad_ai_services::wire::ToolOutcome;

    fn call(tool: &str, args: &str) -> ServiceCall {
        ServiceCall {
            call_id: "c1".into(),
            tool: tool.into(),
            args: args.into(),
        }
    }

    #[test]
    fn the_manifest_declares_one_read_tool() {
        let m = manifest();
        assert_eq!(m.id, "calculator");
        assert!(m.validate().is_ok());
        assert_eq!(m.tools.len(), 1);
        assert_eq!(m.tools[0].name, "eval");
        assert_eq!(m.tools[0].risk, Risk::Read);
    }

    #[test]
    fn eval_succeeds_refuses_bad_args_and_fails_math() {
        let doc = seed::initial();
        let ok = eval_on(&doc, &call("eval", r#"{"expression":"200+10%"}"#));
        assert_eq!(ok.outcome, ToolOutcome::Ok);
        assert!(ok.data.contains("\"value\":220"));
        assert!(ok.data.contains("\"angle\":\"deg\""));

        let refused = eval_on(&doc, &call("eval", r#"{"expression":"1","extra":true}"#));
        assert_eq!(refused.outcome, ToolOutcome::Refused);

        let missing = eval_on(&doc, &call("eval", r#"{}"#));
        assert_eq!(missing.outcome, ToolOutcome::Refused);

        let fail = eval_on(&doc, &call("eval", r#"{"expression":"1/0"}"#));
        assert_eq!(fail.outcome, ToolOutcome::Failed);
        assert!(fail.data.contains("division_by_zero"));

        let unknown = answer(&doc, &call("nope", "{}"));
        assert_eq!(unknown.outcome, ToolOutcome::Refused);
    }

    #[test]
    fn eval_does_not_mutate_the_document() {
        let mut doc = seed::initial();
        doc.session.source = "9".into();
        doc.session.value = 9.0;
        let before = doc.clone();
        let _ = eval_on(&doc, &call("eval", r#"{"expression":"2+2"}"#));
        assert_eq!(doc, before);
    }
    #[test]
    fn tool_refuses_all_engine_limits_and_unknown_tools() {
        let doc = seed::initial();
        for expression in ["(".repeat(100), "1+".repeat(200), "1".repeat(1025)] {
            let args = json::obj(vec![("expression", json::s(&expression))]).to_json();
            assert_eq!(answer(&doc, &call("eval", &args)).outcome, ToolOutcome::Refused);
        }
        assert_eq!(answer(&doc, &call("nope", "{}")).outcome, ToolOutcome::Refused);
    }

}
