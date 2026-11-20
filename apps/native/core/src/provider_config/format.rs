use anyhow::{Result, anyhow};
use serde_json::{Number, Value};

pub(super) fn parse(bytes: &[u8]) -> Result<Value> {
    // The reference reads text with UTF-8 replacement before parsing JSON.
    serde_json::from_str(&String::from_utf8_lossy(bytes))
        .map_err(|_| anyhow!("Invalid provider configuration JSON"))
}

pub(super) fn stringify(value: &Value) -> String {
    let mut output = String::new();
    append(value, &mut output, 0);
    output.push('\n');
    output
}

fn append(value: &Value, output: &mut String, depth: usize) {
    match value {
        Value::Object(object) if !object.is_empty() => {
            // The storage transaction lane has its own private helper. Recurse
            // here so integer-index keys use JS order at every nested object.
            let mut keys: Vec<_> = object.keys().collect();
            keys.sort_by_cached_key(|key| array_index(key).unwrap_or(u32::MAX));
            output.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                separate(output, index, depth + 1);
                string(key, output);
                output.push_str(": ");
                append(&object[key], output, depth + 1);
            }
            separate(output, 0, depth);
            output.push('}');
        }
        Value::Array(array) if !array.is_empty() => {
            output.push('[');
            for (index, value) in array.iter().enumerate() {
                separate(output, index, depth + 1);
                append(value, output, depth + 1);
            }
            separate(output, 0, depth);
            output.push(']');
        }
        Value::Number(value) => output.push_str(&number(value)),
        Value::String(value) => string(value, output),
        scalar => output.push_str(&scalar.to_string()),
    }
}

fn string(value: &str, output: &mut String) {
    output
        .push_str(&serde_json::to_string(value).expect("JSON string serialization is infallible"));
}

fn separate(output: &mut String, index: usize, depth: usize) {
    if index != 0 {
        output.push(',');
    }
    output.push('\n');
    for _ in 0..depth {
        output.push_str("  ");
    }
}

fn array_index(key: &str) -> Option<u32> {
    if (key.len() > 1 && key.starts_with('0')) || !key.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    key.parse::<u32>().ok().filter(|&index| index < u32::MAX)
}

pub(super) fn number(value: &Number) -> String {
    let value = value
        .as_f64()
        .expect("a JSON number is representable as f64");
    if value == 0.0 {
        return "0".into();
    }
    let shortest = format!("{value:e}");
    let (digits, _) = shortest.split_once('e').expect("scientific notation");
    let precision = digits
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len());
    // At equal distance ECMAScript takes the even significand. Rust's fixed
    // precision formatter uses ties-to-even, but can round outside the
    // round-trip interval at a power of two. Keep the shortest form then.
    let rounded = format!("{value:.precision$e}");
    let decimal = if rounded.parse::<f64>() == Ok(value) {
        &rounded
    } else {
        &shortest
    };
    let (digits, power) = decimal.split_once('e').unwrap();
    if (1e-6..1e21).contains(&value.abs()) {
        let (sign, digits) = digits
            .strip_prefix('-')
            .map_or(("", digits), |digits| ("-", digits));
        let digits = digits.replace('.', "");
        let point = power.parse::<i32>().unwrap() + 1;
        if point <= 0 {
            return format!("{sign}0.{}{digits}", "0".repeat(-point as usize));
        }
        let point = point as usize;
        if point >= digits.len() {
            return format!("{sign}{digits}{}", "0".repeat(point - digits.len()));
        }
        return format!("{sign}{}.{}", &digits[..point], &digits[point..]);
    }
    let sign = if power.starts_with('-') { "" } else { "+" };
    format!("{digits}e{sign}{power}")
}
