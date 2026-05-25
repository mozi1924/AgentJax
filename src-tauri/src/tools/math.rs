pub(crate) fn evaluate_math_expression(expr: &str) -> Result<f64, String> {
    let mut chars = expr.chars().peekable();
    let val = parse_add_sub(&mut chars)?;
    if let Some(&c) = chars.peek() {
        return Err(format!("Unexpected character '{}' at end of expression", c));
    }
    Ok(val)
}

fn parse_add_sub<I>(chars: &mut std::iter::Peekable<I>) -> Result<f64, String>
where
    I: Iterator<Item = char>,
{
    let mut val = parse_mul_div(chars)?;
    while let Some(&c) = chars.peek() {
        if c == '+' {
            chars.next();
            val += parse_mul_div(chars)?;
        } else if c == '-' {
            chars.next();
            val -= parse_mul_div(chars)?;
        } else {
            break;
        }
    }
    Ok(val)
}

fn parse_mul_div<I>(chars: &mut std::iter::Peekable<I>) -> Result<f64, String>
where
    I: Iterator<Item = char>,
{
    let mut val = parse_exp(chars)?;
    while let Some(&c) = chars.peek() {
        if c == '*' {
            chars.next();
            val *= parse_exp(chars)?;
        } else if c == '/' {
            chars.next();
            let divisor = parse_exp(chars)?;
            if divisor == 0.0 {
                return Err("Division by zero".to_string());
            }
            val /= divisor;
        } else {
            break;
        }
    }
    Ok(val)
}

fn parse_exp<I>(chars: &mut std::iter::Peekable<I>) -> Result<f64, String>
where
    I: Iterator<Item = char>,
{
    let mut val = parse_primary(chars)?;
    while let Some(&c) = chars.peek() {
        if c == '^' {
            chars.next();
            val = val.powf(parse_primary(chars)?);
        } else {
            break;
        }
    }
    Ok(val)
}

fn parse_primary<I>(chars: &mut std::iter::Peekable<I>) -> Result<f64, String>
where
    I: Iterator<Item = char>,
{
    if let Some(&c) = chars.peek() {
        if c == '(' {
            chars.next();
            let val = parse_add_sub(chars)?;
            if chars.next() != Some(')') {
                return Err("Missing matching closing parenthesis".to_string());
            }
            return Ok(val);
        }

        if c == '-' {
            chars.next();
            return Ok(-parse_primary(chars)?);
        }

        if c == '+' {
            chars.next();
            return parse_primary(chars);
        }

        if c.is_ascii_alphabetic() {
            let mut func = String::new();
            while let Some(&next_c) = chars.peek() {
                if next_c.is_ascii_alphabetic() {
                    func.push(next_c);
                    chars.next();
                } else {
                    break;
                }
            }

            if func == "sqrt" {
                if chars.peek() != Some(&'(') {
                    return Err("sqrt function requires parenthesis, e.g. sqrt(16)".to_string());
                }
                chars.next();
                let val = parse_add_sub(chars)?;
                if chars.next() != Some(')') {
                    return Err("Missing closing parenthesis for sqrt".to_string());
                }
                if val < 0.0 {
                    return Err("Cannot compute square root of a negative number".to_string());
                }
                return Ok(val.sqrt());
            }

            return Err(format!("Unsupported function/variable name '{}'", func));
        }

        if c.is_ascii_digit() || c == '.' {
            let mut num_str = String::new();
            while let Some(&next_c) = chars.peek() {
                if next_c.is_ascii_digit() || next_c == '.' {
                    num_str.push(next_c);
                    chars.next();
                } else {
                    break;
                }
            }

            let num = num_str
                .parse::<f64>()
                .map_err(|e| format!("Failed to parse number '{}': {e}", num_str))?;
            return Ok(num);
        }
    }

    Err("Unexpected end of expression".to_string())
}
