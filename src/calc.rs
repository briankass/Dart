//! Tiny arithmetic evaluator for the launcher's calculator row.
//!
//! Supports + - * / % ^, parentheses, unary minus and decimals.
//! `x` and `×` are accepted as multiply, `÷` as divide.

/// Evaluates `input` if it looks like a calculation (contains at least one
/// operator) and returns the formatted result.
pub fn eval(input: &str) -> Option<String> {
    let s = input.trim().trim_start_matches('=').trim();
    if s.is_empty() || !s.chars().any(|c| "+-*/%^x×÷(".contains(c)) {
        return None;
    }
    // Must start like an expression, so "xbox" or "7-zip" don't count.
    let tokens = tokenize(s)?;
    if !tokens.iter().any(|t| matches!(t, Tok::Op(_))) {
        return None;
    }
    let mut p = Parser { tokens, pos: 0 };
    let v = p.expr()?;
    if p.pos != p.tokens.len() || !v.is_finite() {
        return None;
    }
    Some(format_number(v))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Op(char),
    LParen,
    RParen,
}

fn tokenize(s: &str) -> Option<Vec<Tok>> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '_' => i += 1,
            '0'..='9' | '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == ',') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().filter(|&&c| c != ',').collect();
                out.push(Tok::Num(text.parse().ok()?));
            }
            '+' | '-' | '*' | '/' | '%' | '^' => {
                out.push(Tok::Op(c));
                i += 1;
            }
            'x' | 'X' | '×' => {
                out.push(Tok::Op('*'));
                i += 1;
            }
            '÷' => {
                out.push(Tok::Op('/'));
                i += 1;
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            _ => return None,
        }
    }
    Some(out)
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<Tok> {
        self.tokens.get(self.pos).copied()
    }

    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        while let Some(Tok::Op(op @ ('+' | '-'))) = self.peek() {
            self.pos += 1;
            let r = self.term()?;
            v = if op == '+' { v + r } else { v - r };
        }
        Some(v)
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        while let Some(Tok::Op(op @ ('*' | '/' | '%'))) = self.peek() {
            self.pos += 1;
            let r = self.unary()?;
            v = match op {
                '*' => v * r,
                '/' => v / r,
                _ => v % r,
            };
        }
        Some(v)
    }

    fn unary(&mut self) -> Option<f64> {
        match self.peek() {
            Some(Tok::Op('-')) => {
                self.pos += 1;
                Some(-self.unary()?)
            }
            Some(Tok::Op('+')) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.primary()?;
        if let Some(Tok::Op('^')) = self.peek() {
            self.pos += 1;
            let exp = self.unary()?; // right-associative
            return Some(base.powf(exp));
        }
        Some(base)
    }

    fn primary(&mut self) -> Option<f64> {
        match self.peek()? {
            Tok::Num(n) => {
                self.pos += 1;
                Some(n)
            }
            Tok::LParen => {
                self.pos += 1;
                let v = self.expr()?;
                if self.peek() == Some(Tok::RParen) {
                    self.pos += 1;
                }
                // A missing ")" at the end is forgiven while typing.
                Some(v)
            }
            _ => None,
        }
    }
}

fn format_number(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.10}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::eval;

    #[test]
    fn basics() {
        assert_eq!(eval("2+2").as_deref(), Some("4"));
        assert_eq!(eval("2 * 21").as_deref(), Some("42"));
        assert_eq!(eval("10/4").as_deref(), Some("2.5"));
        assert_eq!(eval("(1+2)*3").as_deref(), Some("9"));
        assert_eq!(eval("2^10").as_deref(), Some("1024"));
        assert_eq!(eval("2^3^2").as_deref(), Some("512"));
        assert_eq!(eval("-3+5").as_deref(), Some("2"));
        assert_eq!(eval("7 % 3").as_deref(), Some("1"));
        assert_eq!(eval("1,000 x 3").as_deref(), Some("3000"));
        assert_eq!(eval("=1/3").as_deref(), Some("0.3333333333"));
        assert_eq!(eval("(2+3").as_deref(), Some("5"));
    }

    #[test]
    fn not_a_calculation() {
        assert_eq!(eval("notepad"), None);
        assert_eq!(eval("7-zip"), None);
        assert_eq!(eval("xbox"), None);
        assert_eq!(eval("42"), None);
        assert_eq!(eval("1/0"), None);
        assert_eq!(eval("2+"), None);
        assert_eq!(eval(""), None);
    }
}
