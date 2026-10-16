"""A small arithmetic evaluator: numbers, + - * / and parentheses."""
import re

TOKEN = re.compile(r"\s*(?:(\d+\.?\d*)|(.))")


def tokenize(text):
    tokens = []
    for number, op in TOKEN.findall(text):
        if number:
            tokens.append(float(number) if "." in number else int(number))
        elif op.strip():
            if op not in "+-*/()":
                raise SyntaxError(f"unexpected {op!r}")
            tokens.append(op)
    return tokens


class Parser:
    def __init__(self, tokens):
        self.tokens = tokens
        self.i = 0

    def peek(self):
        return self.tokens[self.i] if self.i < len(self.tokens) else None

    def take(self):
        tok = self.peek()
        self.i += 1
        return tok

    def expr(self):
        value = self.term()
        while self.peek() in ("+", "-"):
            if self.take() == "+":
                value += self.term()
            else:
                value -= self.term()
        return value

    def term(self):
        value = self.factor()
        while self.peek() in ("*", "/"):
            if self.take() == "*":
                value *= self.factor()
            else:
                value /= self.factor()
        return value

    def factor(self):
        tok = self.take()
        if tok == "(":
            value = self.expr()
            if self.take() != ")":
                raise SyntaxError("expected )")
            return value
        if isinstance(tok, (int, float)):
            return tok
        raise SyntaxError(f"unexpected {tok!r}")


def evaluate(text):
    parser = Parser(tokenize(text))
    value = parser.expr()
    if parser.peek() is not None:
        raise SyntaxError(f"unexpected {parser.peek()!r}")
    return value
