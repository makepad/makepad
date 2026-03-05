import sys

with open('src/lighting.rs', 'r') as f:
    content = f.read()

content = content.replace(
    "if _p.x == 1.7187748353e-2 && _p.y == 1.2128777161e-2 { println!(\"AO step: t={:e}, de={:e}, s_tmp={}\", t, de, s_tmp); }",
    ""
)

with open('src/lighting.rs', 'w') as f:
    f.write(content)
