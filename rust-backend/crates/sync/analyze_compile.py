import os
import re
from collections import defaultdict

# File to analyze
file_path = "C:/Users/singh/BitChord-PC-Continued/rust-backend/crates/sync/src/lib.rs"

# Read file content
with open(file_path, 'r') as f:
    content = f.read()

lines = content.split('\n')
total_lines = len(lines)

# Find all imports
import_pattern = re.compile(r'^use\s+(?:(?:[a-zA-Z0-9_]+::)*([a-zA-Z0-9_]+)(?:\s+as\s+[a-zA-Z0-9_]+)?)\s*(?:\{[^}]*\}|;|,$)')

# Find all structs/enums/functions
struct_pattern = re.compile(r'(?:pub\s*)?(struct|enum|fn|mod|const|type)\s+([a-zA-Z0-9_]+)')

# Find all used identifiers
identifier_pattern = re.compile(r'\b([a-zA-Z0-9_]+)\b')

# Find all imports
imports = []
for i, line in enumerate(lines, 1):
    if line.strip().startswith('use '):
        imports.append((i, line.strip()))

# Find all public items
public_items = []
for i, line in enumerate(lines, 1):
    if re.search(r'pub\s+(?:struct|enum|fn|const|type|mod)', line):
        # Extract the name
        m = re.search(r'pub\s+(?:struct|enum|fn|const|type|mod)\s+([a-zA-Z0-9_]+)', line)
        if m:
            public_items.append((i, m.group(1), line.strip()))

# Find all impl blocks
impl_blocks = []
for i, line in enumerate(lines, 1):
    if line.strip().startswith('impl '):
        impl_blocks.append((i, line.strip()))

print(f"=== Analysis of {file_path} ===")
print(f"Total lines: {total_lines}")
print(f"Imports found: {len(imports)}")
print("\n--- Imports ---")
for line_num, import_line in imports:
    print(f"{line_num}: {import_line}")

print("\n--- Public Items ---")
for line_num, name, item_line in public_items:
    print(f"{line_num}: {name} - {item_line}")

print("\n--- Impl Blocks ---")
for line_num, impl_line in impl_blocks:
    print(f"{line_num}: {impl_line}")

# Check for unused imports by comparing imported names vs used identifiers
all_imports = set()
for _, import_line in imports:
    # Extract imported items
    # Handle simple imports like "use std::collections::HashMap;"
    # Handle "use anyhow::Result;"
    # Handle "use serde::{Deserialize, Serialize};"
    import_matches = re.findall(r'([a-zA-Z0-9_]+)(?:\s+as\s+[a-zA-Z0-9_]+)?', import_line)
    for imp in import_matches:
        all_imports.add(imp)

# Find all identifiers used in the file
used_ids = set()
for line in lines:
    # Find all identifiers
    for match in identifier_pattern.finditer(line):
        ident = match.group(1)
        # Skip keywords and common rust builtins
        if ident in ['pub', 'use', 'struct', 'enum', 'fn', 'impl', 'trait', 'mod', 'const', 'type', 'self', 'Self']:
            continue
        used_ids.add(ident)

# Find unused imports
print("\n--- Potential Unused Imports Analysis ---")
unused_imports = all_imports - used_ids
if unused_imports:
    print("WARNING: Potential unused imports detected:")
    for imp in sorted(unused_imports):
        print(f"  - {imp}")
else:
    print("No unused imports detected")

# Check for dead code patterns
print("\n--- Dead Code Analysis ---")
# Look for functions that might be unused
function_pattern = re.compile(r'fn\s+([a-zA-Z0-9_]+)\s*\(')
functions = []
for i, line in enumerate(lines, 1):
    m = function_pattern.search(line)
    if m:
        functions.append((i, m.group(1)))

# Check if any public functions are unused
public_functions = [(i, name) for i, name in functions if any(f'fn {name}' in item_line or name in item_line for _, _, item_line in public_items)]
print(f"Public functions found: {len(public_functions)}")
for i, name in public_functions:
    # Check if this function is called anywhere
    called = False
    for line in lines:
        if f'{name}(' in line and not line.strip().startswith('fn'):
            called = True
            break
    if not called:
        print(f"WARNING: Public function '{name}' at line {i} appears unused")

# Check Cargo.toml dependencies vs usage
print("\n--- Cargo.toml Dependencies Analysis ---")
cargo_path = "C:/Users/singh/BitChord-PC-Continued/rust-backend/crates/sync/Cargo.toml"
with open(cargo_path, 'r') as f:
    cargo_content = f.read()

deps = re.findall(r'([a-zA-Z0-9_-]+)\s*=\s*\{[^}]*\}|([a-zA-Z0-9_-]+)\s*=\s*"[^"]*"|([a-zA-Z0-9_-]+)\s*=\s*true|([a-zA-Z0-9_-]+)\s*=\s*\{ path = ".*/([a-zA-Z0-9_-]+)"\}', cargo_content)
print(f"Dependencies declared: {len([d for d in deps if any(d)])}")

# Check for type mismatches
print("\n--- Type Mismatch Analysis ---")
# Look for HashMap usage vs expected types
hashmap_usage = re.findall(r'HashMap<([^,]+),\s*([^>]+)>', content)
for match in hashmap_usage:
    key_type, value_type = match
    print(f"HashMap<{key_type}, {value_type}> found - verify types are correct")

print("\n=== Analysis Complete ===")