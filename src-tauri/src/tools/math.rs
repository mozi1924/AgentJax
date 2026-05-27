use statrs::function::{beta, erf, factorial, gamma, harmonic};
use std::f64;

const INTEGER_TOLERANCE: f64 = 1e-9;
pub const GOLDEN_RATIO: f64 = 1.618_033_988_749_895;

/// Validates integer-style function inputs (factorial, nCr, harmonic, etc.)
/// while keeping the evaluator API in simple `f64` terms.
fn as_non_negative_integer(value: f64, function_name: &str) -> Result<u64, String> {
    if !value.is_finite() {
        return Err(format!(
            "{function_name} requires a finite non-negative integer input"
        ));
    }

    if value < 0.0 {
        return Err(format!(
            "{function_name} requires a non-negative integer input"
        ));
    }

    let rounded = value.round();
    if (value - rounded).abs() > INTEGER_TOLERANCE {
        return Err(format!(
            "{function_name} requires an integer input, received {value}"
        ));
    }

    if rounded > u64::MAX as f64 {
        return Err(format!(
            "{function_name} input is too large to represent safely"
        ));
    }

    Ok(rounded as u64)
}

pub fn safe_gamma(value: f64) -> Result<f64, String> {
    if value <= 0.0 && (value - value.round()).abs() <= INTEGER_TOLERANCE {
        return Err("gamma is undefined for zero and negative integers".to_string());
    }

    let result = gamma::gamma(value);
    if !result.is_finite() {
        return Err("gamma overflowed or produced a non-finite result".to_string());
    }

    Ok(result)
}

pub fn safe_ln_gamma(value: f64) -> Result<f64, String> {
    if !value.is_finite() {
        return Err("ln_gamma requires a finite input".to_string());
    }
    Ok(gamma::ln_gamma(value))
}

pub fn safe_digamma(value: f64) -> Result<f64, String> {
    if !value.is_finite() {
        return Err("digamma requires a finite input".to_string());
    }
    Ok(gamma::digamma(value))
}

pub fn safe_erf(value: f64) -> Result<f64, String> {
    if !value.is_finite() {
        return Err("erf requires a finite input".to_string());
    }
    Ok(erf::erf(value))
}

pub fn safe_erfc(value: f64) -> Result<f64, String> {
    if !value.is_finite() {
        return Err("erfc requires a finite input".to_string());
    }
    Ok(erf::erfc(value))
}

pub fn safe_erf_inv(value: f64) -> Result<f64, String> {
    if !(-1.0..1.0).contains(&value) {
        return Err("erf_inv requires -1 < x < 1".to_string());
    }

    Ok(erf::erf_inv(value))
}

pub fn safe_erfc_inv(value: f64) -> Result<f64, String> {
    if !(0.0..2.0).contains(&value) {
        return Err("erfc_inv requires 0 < x < 2".to_string());
    }

    Ok(erf::erfc_inv(value))
}

pub fn safe_beta(a: f64, b: f64) -> Result<f64, String> {
    match beta::checked_beta(a, b) {
        Ok(result) => Ok(result),
        Err(_) => Err("beta requires both arguments to be greater than zero".to_string()),
    }
}

pub fn safe_ln_beta(a: f64, b: f64) -> Result<f64, String> {
    match beta::checked_ln_beta(a, b) {
        Ok(result) => Ok(result),
        Err(_) => Err("ln_beta requires both arguments to be greater than zero".to_string()),
    }
}

pub fn safe_factorial(value: f64) -> Result<f64, String> {
    let n = as_non_negative_integer(value, "factorial")?;

    if n as usize > factorial::MAX_FACTORIAL {
        return Err(format!(
            "factorial overflows f64 for inputs larger than {}",
            factorial::MAX_FACTORIAL
        ));
    }

    Ok(factorial::factorial(n))
}

pub fn safe_ln_factorial(value: f64) -> Result<f64, String> {
    let n = as_non_negative_integer(value, "ln_factorial")?;
    Ok(factorial::ln_factorial(n))
}

pub fn safe_ncr(n: f64, r: f64) -> Result<f64, String> {
    let n = as_non_negative_integer(n, "ncr")?;
    let r = as_non_negative_integer(r, "ncr")?;
    Ok(factorial::binomial(n, r))
}

pub fn safe_npr(n: f64, r: f64) -> Result<f64, String> {
    let n = as_non_negative_integer(n, "npr")?;
    let r = as_non_negative_integer(r, "npr")?;

    if r > n {
        return Ok(0.0);
    }

    Ok((factorial::ln_factorial(n) - factorial::ln_factorial(n - r)).exp())
}

pub fn safe_logistic(value: f64) -> Result<f64, String> {
    Ok(1.0 / (1.0 + (-value).exp()))
}

pub fn safe_logit(value: f64) -> Result<f64, String> {
    if !(0.0..1.0).contains(&value) {
        return Err("logit requires 0 < p < 1".to_string());
    }

    Ok((value / (1.0 - value)).ln())
}

pub fn safe_harmonic(value: f64) -> Result<f64, String> {
    let n = as_non_negative_integer(value, "harmonic")?;
    Ok(harmonic::harmonic(n))
}

pub fn safe_gen_harmonic(n: f64, order: f64) -> Result<f64, String> {
    let n = as_non_negative_integer(n, "gen_harmonic")?;

    if !order.is_finite() {
        return Err("gen_harmonic requires a finite order parameter".to_string());
    }

    Ok(harmonic::gen_harmonic(n, order))
}
