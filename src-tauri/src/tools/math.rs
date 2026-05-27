use meval::{Context, Expr};
use statrs::function::{beta, erf, factorial, gamma, harmonic};
use std::cell::RefCell;
use std::f64;
use std::str::FromStr;

thread_local! {
    /// Carries domain/overflow failures from custom math helpers back to the
    /// top-level evaluator so the calculator tool can return actionable errors.
    static LAST_MATH_ERROR: RefCell<Option<String>> = RefCell::new(None);
}

const INTEGER_TOLERANCE: f64 = 1e-9;
const GOLDEN_RATIO: f64 = 1.618_033_988_749_895;

/// Evaluates a math expression using `meval` for parsing and `statrs` for
/// higher-level special/statistical functions.
pub(crate) fn evaluate_math_expression(expr: &str) -> Result<f64, String> {
    clear_last_math_error();

    let parsed =
        Expr::from_str(expr).map_err(|err| format!("Failed to parse expression: {err}"))?;
    let result = parsed
        .eval_with_context(build_math_context())
        .map_err(|err| format!("Failed to evaluate expression: {err}"))?;

    if let Some(err) = take_last_math_error() {
        return Err(err);
    }

    if !result.is_finite() {
        return Err(
            "Expression evaluated to a non-finite result. Check for invalid domains, division by zero, or numeric overflow."
                .to_string(),
        );
    }

    Ok(result)
}

/// Builds the calculator context with safe helpers layered on top of `meval`'s
/// standard arithmetic/trigonometric functions.
fn build_math_context<'a>() -> Context<'a> {
    let mut ctx = Context::new();

    ctx.var("tau", std::f64::consts::TAU);
    ctx.var("phi", GOLDEN_RATIO);

    ctx.func("gamma", safe_gamma);
    ctx.func("ln_gamma", gamma::ln_gamma);
    ctx.func("digamma", gamma::digamma);

    ctx.func("erf", erf::erf);
    ctx.func("erfc", erf::erfc);
    ctx.func("erf_inv", safe_erf_inv);
    ctx.func("erfc_inv", safe_erfc_inv);

    ctx.func2("beta", safe_beta);
    ctx.func2("ln_beta", safe_ln_beta);

    ctx.func("factorial", safe_factorial);
    ctx.func("ln_factorial", safe_ln_factorial);
    ctx.func2("ncr", safe_ncr);
    ctx.func2("npr", safe_npr);

    ctx.func("logistic", safe_logistic);
    ctx.func("logit", safe_logit);

    ctx.func("harmonic", safe_harmonic);
    ctx.func2("gen_harmonic", safe_gen_harmonic);

    ctx.funcn("sum", |xs| xs.iter().sum(), 1..);
    ctx.funcn("mean", |xs| xs.iter().sum::<f64>() / xs.len() as f64, 1..);
    ctx.funcn("product", |xs| xs.iter().product(), 1..);

    ctx
}

fn clear_last_math_error() {
    LAST_MATH_ERROR.with(|slot| {
        slot.borrow_mut().take();
    });
}

fn take_last_math_error() -> Option<String> {
    LAST_MATH_ERROR.with(|slot| slot.borrow_mut().take())
}

fn set_last_math_error(message: impl Into<String>) {
    LAST_MATH_ERROR.with(|slot| {
        *slot.borrow_mut() = Some(message.into());
    });
}

/// Validates integer-style function inputs (factorial, nCr, harmonic, etc.)
/// while keeping the evaluator API in simple `f64` terms.
fn as_non_negative_integer(value: f64, function_name: &str) -> Option<u64> {
    if !value.is_finite() {
        set_last_math_error(format!(
            "{function_name} requires a finite non-negative integer input"
        ));
        return None;
    }

    if value < 0.0 {
        set_last_math_error(format!(
            "{function_name} requires a non-negative integer input"
        ));
        return None;
    }

    let rounded = value.round();
    if (value - rounded).abs() > INTEGER_TOLERANCE {
        set_last_math_error(format!(
            "{function_name} requires an integer input, received {value}"
        ));
        return None;
    }

    if rounded > u64::MAX as f64 {
        set_last_math_error(format!(
            "{function_name} input is too large to represent safely"
        ));
        return None;
    }

    Some(rounded as u64)
}

fn safe_gamma(value: f64) -> f64 {
    if value <= 0.0 && (value - value.round()).abs() <= INTEGER_TOLERANCE {
        set_last_math_error("gamma is undefined for zero and negative integers");
        return f64::NAN;
    }

    let result = gamma::gamma(value);
    if !result.is_finite() {
        set_last_math_error("gamma overflowed or produced a non-finite result");
        return f64::NAN;
    }

    result
}

fn safe_erf_inv(value: f64) -> f64 {
    if !(-1.0..1.0).contains(&value) {
        set_last_math_error("erf_inv requires -1 < x < 1");
        return f64::NAN;
    }

    erf::erf_inv(value)
}

fn safe_erfc_inv(value: f64) -> f64 {
    if !(0.0..2.0).contains(&value) {
        set_last_math_error("erfc_inv requires 0 < x < 2");
        return f64::NAN;
    }

    erf::erfc_inv(value)
}

fn safe_beta(a: f64, b: f64) -> f64 {
    match beta::checked_beta(a, b) {
        Ok(result) => result,
        Err(_) => {
            set_last_math_error("beta requires both arguments to be greater than zero");
            f64::NAN
        }
    }
}

fn safe_ln_beta(a: f64, b: f64) -> f64 {
    match beta::checked_ln_beta(a, b) {
        Ok(result) => result,
        Err(_) => {
            set_last_math_error("ln_beta requires both arguments to be greater than zero");
            f64::NAN
        }
    }
}

fn safe_factorial(value: f64) -> f64 {
    let Some(n) = as_non_negative_integer(value, "factorial") else {
        return f64::NAN;
    };

    if n as usize > factorial::MAX_FACTORIAL {
        set_last_math_error(format!(
            "factorial overflows f64 for inputs larger than {}",
            factorial::MAX_FACTORIAL
        ));
        return f64::NAN;
    }

    factorial::factorial(n)
}

fn safe_ln_factorial(value: f64) -> f64 {
    let Some(n) = as_non_negative_integer(value, "ln_factorial") else {
        return f64::NAN;
    };

    factorial::ln_factorial(n)
}

fn safe_ncr(n: f64, r: f64) -> f64 {
    let Some(n) = as_non_negative_integer(n, "ncr") else {
        return f64::NAN;
    };
    let Some(r) = as_non_negative_integer(r, "ncr") else {
        return f64::NAN;
    };

    factorial::binomial(n, r)
}

fn safe_npr(n: f64, r: f64) -> f64 {
    let Some(n) = as_non_negative_integer(n, "npr") else {
        return f64::NAN;
    };
    let Some(r) = as_non_negative_integer(r, "npr") else {
        return f64::NAN;
    };

    if r > n {
        return 0.0;
    }

    (factorial::ln_factorial(n) - factorial::ln_factorial(n - r)).exp()
}

fn safe_logistic(value: f64) -> f64 {
    1.0 / (1.0 + (-value).exp())
}

fn safe_logit(value: f64) -> f64 {
    if !(0.0..1.0).contains(&value) {
        set_last_math_error("logit requires 0 < p < 1");
        return f64::NAN;
    }

    (value / (1.0 - value)).ln()
}

fn safe_harmonic(value: f64) -> f64 {
    let Some(n) = as_non_negative_integer(value, "harmonic") else {
        return f64::NAN;
    };

    harmonic::harmonic(n)
}

fn safe_gen_harmonic(n: f64, order: f64) -> f64 {
    let Some(n) = as_non_negative_integer(n, "gen_harmonic") else {
        return f64::NAN;
    };

    if !order.is_finite() {
        set_last_math_error("gen_harmonic requires a finite order parameter");
        return f64::NAN;
    }

    harmonic::gen_harmonic(n, order)
}
