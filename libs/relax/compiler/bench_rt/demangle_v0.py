"""Minimal Rust v0 symbol demangler (rustc's `_R` mangling; RFC 2603) for asmdiff/rank: paths, impls,
generic args and types; enough to name functions like rustc-demangle does (no hashes)."""

BASIC = {"a": "i8", "b": "bool", "c": "char", "d": "f64", "e": "str", "f": "f32", "h": "u8", "i": "isize", "j": "usize",
         "l": "i32", "m": "u32", "n": "i128", "o": "u128", "s": "i16", "t": "u16", "u": "()", "v": "...", "x": "i64",
         "y": "u64", "z": "!", "p": "_"}


class P:
    def __init__(self, s):
        self.s = s
        self.i = 0

    def peek(self):
        return self.s[self.i] if self.i < len(self.s) else ""

    def eat(self, c):
        if self.peek() == c:
            self.i += 1
            return True
        return False

    def next(self):
        c = self.s[self.i]
        self.i += 1
        return c

    def b62(self):
        # base-62 number terminated by '_' ("_" alone = 0)
        if self.eat("_"):
            return 0
        n = 0
        while True:
            c = self.next()
            if c == "_":
                return n + 1
            if c.isdigit():
                d = ord(c) - 48
            elif c.islower():
                d = ord(c) - 97 + 10
            else:
                d = ord(c) - 65 + 36
            n = n * 62 + d

    def dec(self):
        j = self.i
        while self.peek().isdigit():
            self.i += 1
        return int(self.s[j:self.i])

    def ident(self):
        if self.eat("s"):
            self.b62()
        puny = self.eat("u")
        n = self.dec()
        if n > 0:
            self.eat("_")
        t = self.s[self.i:self.i + n]
        self.i += n
        return t

    def backref(self, f):
        pos = self.b62()
        save = self.i
        self.i = pos  # positions count from after "_R"
        r = f()
        self.i = save
        return r

    def path(self):
        c = self.next()
        if c == "C":
            return self.ident()
        if c == "M":
            self.impl_path()
            return "<" + self.type() + ">"
        if c == "X":
            self.impl_path()
            t = self.type()
            return "<" + t + " as " + self.path() + ">"
        if c == "Y":
            t = self.type()
            return "<" + t + " as " + self.path() + ">"
        if c == "N":
            ns = self.next()
            p = self.path()
            name = self.ident()
            if ns == "C":
                return p + "::{closure}"
            if ns.isupper():
                return p + "::{" + ns.lower() + "}" + ("" if not name else name)
            return p + "::" + name
        if c == "I":
            p = self.path()
            args = []
            while not self.eat("E"):
                args.append(self.generic_arg())
            args = [a for a in args if a is not None]
            return p + ("<" + ", ".join(args) + ">" if args else "")
        if c == "B":
            return self.backref(self.path)
        raise ValueError("path " + c)

    def impl_path(self):
        if self.eat("s"):
            self.b62()
        return self.path()

    def generic_arg(self):
        if self.eat("L"):
            self.b62()
            return None
        if self.eat("K"):
            return self.const()
        return self.type()

    def const(self):
        if self.eat("p"):
            return "_"
        if self.eat("B"):
            return self.backref(self.const)
        t = self.next()
        neg = self.eat("n")
        j = self.i
        while self.peek() != "_":
            self.i += 1
        h = self.s[j:self.i]
        self.i += 1
        return ("-" if neg else "") + str(int(h, 16) if h else 0)

    def type(self):
        c = self.peek()
        if c in BASIC:
            self.i += 1
            return BASIC[c]
        self.i += 1
        if c == "A":
            t = self.type()
            return "[%s; %s]" % (t, self.const())
        if c == "S":
            return "[" + self.type() + "]"
        if c == "T":
            ts = []
            while not self.eat("E"):
                ts.append(self.type())
            return "(" + ", ".join(ts) + ("," if len(ts) == 1 else "") + ")"
        if c in "RQ":
            if self.eat("L"):
                self.b62()
            return ("&" if c == "R" else "&mut ") + self.type()
        if c in "PO":
            return ("*const " if c == "P" else "*mut ") + self.type()
        if c == "F":
            if self.eat("G"):
                self.b62()
            self.eat("U")
            if self.eat("K"):
                if not self.eat("C"):
                    self.ident()
            ts = []
            while not self.eat("E"):
                ts.append(self.type())
            r = self.type()
            return "fn(" + ", ".join(ts) + ") -> " + r
        if c == "D":
            if self.eat("G"):
                self.b62()
            bs = []
            while not self.eat("E"):
                p = self.path()
                while self.eat("p"):
                    self.ident()
                    self.type()
                bs.append(p)
            if self.eat("L"):
                self.b62()
            return "dyn " + " + ".join(bs)
        if c == "B":
            return self.backref(self.type)
        self.i -= 1
        return self.path()


def demangle(sym):
    s = sym.lstrip("_")
    if not s.startswith("R"):
        return None
    try:
        p = P(s[1:])
        if p.peek().isdigit():
            p.dec()
        return p.path()
    except (ValueError, IndexError):
        return None
