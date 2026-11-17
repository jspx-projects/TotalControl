//! Argument expressions of the controller definition format.
//!
//! `{ "expr": text }` is integer arithmetic evaluated per message, with the
//! address variables `channel`, `note`, `cc`, `value` and `strip`. The
//! grammar is `+ - * / %`, parentheses and integer literals; division
//! truncates toward zero. See `TOTALCONTROL-CONTROLLER-SPEC.md` §4.9.1.

/// The address variables one message provides.
#[derive(Debug, Clone, Copy, Default)]
pub struct Vars {
    pub channel: i64,
    pub note: i64,
    pub cc: i64,
    pub value: i64,
    pub strip: i64,
}

/// A parsed expression, ready to evaluate per message.
#[derive(Debug, Clone)]
pub struct Expr {
    ops: Vec<Op>,
}

#[derive(Debug, Clone)]
enum Op {
    Lit(i64),
    Var(u8),
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Neg,
}

const VAR_NAMES: [&str; 5] = ["channel", "note", "cc", "value", "strip"];

impl Expr {
    /// Parse the expression text; the error names the offending position.
    pub fn parse(text: &str) -> Result<Self, String> {
        let tokens = tokenize(text)?;
        let mut pos = 0;
        let mut ops = Vec::new();
        parse_sum(&tokens, &mut pos, &mut ops)?;
        if pos != tokens.len() {
            return Err(format!("unexpected token at position {pos}"));
        }
        Ok(Self { ops })
    }

    /// Evaluate against one message's variables. Division by zero yields 0.
    pub fn eval(&self, vars: &Vars) -> i64 {
        let mut stack: Vec<i64> = Vec::new();
        for op in &self.ops {
            match op {
                Op::Lit(v) => stack.push(*v),
                Op::Var(i) => stack.push(match i {
                    0 => vars.channel,
                    1 => vars.note,
                    2 => vars.cc,
                    3 => vars.value,
                    _ => vars.strip,
                }),
                Op::Neg => {
                    let a = stack.pop().unwrap_or(0);
                    stack.push(-a);
                }
                Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Rem => {
                    let b = stack.pop().unwrap_or(0);
                    let a = stack.pop().unwrap_or(0);
                    stack.push(match op {
                        Op::Add => a.wrapping_add(b),
                        Op::Sub => a.wrapping_sub(b),
                        Op::Mul => a.wrapping_mul(b),
                        Op::Div => {
                            if b == 0 {
                                0
                            } else {
                                a / b
                            }
                        }
                        _ => {
                            if b == 0 {
                                0
                            } else {
                                a % b
                            }
                        }
                    });
                }
            }
        }
        stack.pop().unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Lit(i64),
    Var(u8),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Open,
    Close,
}

fn tokenize(text: &str) -> Result<Vec<Token>, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b' ' | b'\t' => i += 1,
            b'+' => {
                out.push(Token::Plus);
                i += 1;
            }
            b'-' => {
                out.push(Token::Minus);
                i += 1;
            }
            b'*' => {
                out.push(Token::Star);
                i += 1;
            }
            b'/' => {
                out.push(Token::Slash);
                i += 1;
            }
            b'%' => {
                out.push(Token::Percent);
                i += 1;
            }
            b'(' => {
                out.push(Token::Open);
                i += 1;
            }
            b')' => {
                out.push(Token::Close);
                i += 1;
            }
            b'0'..=b'9' => {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let lit = text[start..i]
                    .parse::<i64>()
                    .map_err(|_| format!("literal out of range at position {start}"))?;
                out.push(Token::Lit(lit));
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
                {
                    i += 1;
                }
                let name = &text[start..i];
                let idx = VAR_NAMES
                    .iter()
                    .position(|n| *n == name)
                    .ok_or_else(|| format!("unknown variable '{name}'"))?;
                out.push(Token::Var(idx as u8));
            }
            _ => return Err(format!("unexpected character at position {i}")),
        }
    }
    Ok(out)
}

// Recursive descent compiled to postfix ops: sum -> product -> unary -> atom.
fn parse_sum(tokens: &[Token], pos: &mut usize, ops: &mut Vec<Op>) -> Result<(), String> {
    parse_product(tokens, pos, ops)?;
    while matches!(tokens.get(*pos), Some(Token::Plus | Token::Minus)) {
        let plus = tokens[*pos] == Token::Plus;
        *pos += 1;
        parse_product(tokens, pos, ops)?;
        ops.push(if plus { Op::Add } else { Op::Sub });
    }
    Ok(())
}

fn parse_product(tokens: &[Token], pos: &mut usize, ops: &mut Vec<Op>) -> Result<(), String> {
    parse_unary(tokens, pos, ops)?;
    while matches!(
        tokens.get(*pos),
        Some(Token::Star | Token::Slash | Token::Percent)
    ) {
        let op = match tokens[*pos] {
            Token::Star => Op::Mul,
            Token::Slash => Op::Div,
            _ => Op::Rem,
        };
        *pos += 1;
        parse_unary(tokens, pos, ops)?;
        ops.push(op);
    }
    Ok(())
}

fn parse_unary(tokens: &[Token], pos: &mut usize, ops: &mut Vec<Op>) -> Result<(), String> {
    if matches!(tokens.get(*pos), Some(Token::Minus)) {
        *pos += 1;
        parse_unary(tokens, pos, ops)?;
        ops.push(Op::Neg);
        return Ok(());
    }
    match tokens.get(*pos) {
        Some(Token::Lit(v)) => {
            ops.push(Op::Lit(*v));
            *pos += 1;
            Ok(())
        }
        Some(Token::Var(i)) => {
            ops.push(Op::Var(*i));
            *pos += 1;
            Ok(())
        }
        Some(Token::Open) => {
            *pos += 1;
            parse_sum(tokens, pos, ops)?;
            if tokens.get(*pos) != Some(&Token::Close) {
                return Err("missing closing parenthesis".into());
            }
            *pos += 1;
            Ok(())
        }
        _ => Err(format!("expected a value at position {pos}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(text: &str, vars: Vars) -> i64 {
        Expr::parse(text).unwrap().eval(&vars)
    }

    #[test]
    fn the_row_fader_law() {
        let expr = Expr::parse(
            "((channel - 1) / 4) * 64 + ((channel - 1) % 4) * 16 + (cc - 102)",
        )
        .unwrap();
        // Channel 1, CC 102: row 0, channel 0.
        assert_eq!(expr.eval(&Vars { channel: 1, cc: 102, ..Default::default() }), 0);
        // Channel 5, CC 102: row 1, channel 0.
        assert_eq!(expr.eval(&Vars { channel: 5, cc: 102, ..Default::default() }), 64);
        // Channel 9, CC 117: row 2, channel 15.
        assert_eq!(expr.eval(&Vars { channel: 9, cc: 117, ..Default::default() }), 143);
        // Channel 4, CC 103: row 0, channel 49.
        assert_eq!(expr.eval(&Vars { channel: 4, cc: 103, ..Default::default() }), 49);
    }

    #[test]
    fn precedence_and_parentheses() {
        let v = Vars::default();
        assert_eq!(eval("2 + 3 * 4", v), 14);
        assert_eq!(eval("(2 + 3) * 4", v), 20);
        assert_eq!(eval("7 / 2", v), 3);
        assert_eq!(eval("7 % 3", v), 1);
        assert_eq!(eval("-5 + 2", v), -3);
        assert_eq!(eval("1 / 0", v), 0);
    }

    #[test]
    fn variables_and_strip() {
        let vars = Vars { channel: 13, note: 63, cc: 104, value: 64, strip: 2 };
        assert_eq!(eval("channel - 1", vars), 12);
        assert_eq!(eval("note - 62", vars), 1);
        assert_eq!(eval("(cc - 104) / 2", vars), 0);
        assert_eq!(eval("strip", vars), 2);
    }

    #[test]
    fn rejects_unknown_names_and_syntax() {
        assert!(Expr::parse("foo + 1").is_err());
        assert!(Expr::parse("1 +").is_err());
        assert!(Expr::parse("(1 + 2").is_err());
        assert!(Expr::parse("1 2").is_err());
        assert!(Expr::parse("").is_err());
    }
}