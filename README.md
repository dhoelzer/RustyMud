# ldmud-rs

A from-scratch Rust reimplementation of the [LDMud](https://github.com/ldmud/ldmud)
LPC driver, written by studying the original C sources (`src/`) and its bundled
mudlib/docs (`mudlib/`, `doc/`). Like the original, the driver runs *LPC* code:
gameobjects written in a C-like language are compiled at load time and execute
inside the driver, which provides the efuns, the interactive backend
(telnet/TCP), and the master-object security hooks.

## Layout

| module        | role (original analogue)                              |
|---------------|-------------------------------------------------------|
| `pp.rs`       | C-style preprocessor: `#include/#define/#if...`       |
| `lex.rs`      | LPC lexer (incl. `#'` quotes, `(:` `:)` inline closures) |
| `parse.rs`    | recursive-descent parser → AST                        |
| `ast.rs`      | AST types, `Compiled` constant pool                   |
| `program.rs`  | program objects, inheritance flattening               |
| `value.rs`    | svalue model: int/float/string/array/mapping/closure/object |
| `vm.rs`       | interpreter, object table, frames, master hooks, driver hooks, command loop |
| `eval.rs`     | expression evaluator (incl. LPC sscanf backtracker)   |
| `efuns.rs`    | ~150 driver builtins (write/sprintf/arrays/mappings/objects/interactivity…) |
| `loader.rs`   | file→program pipeline, master boot, nonblocking TCP backend |
| `main.rs`     | driver CLI (`-m mudlibdir`, `-M master.c`, `port`)    |

`examples/mudlib/` holds a minimal runnable mudlib (`master.c` + `obj/player.c`)
mirroring the role of LDMud's own `mudlib/` tree in the upstream repo.

## Running

```sh
cargo run -- -m examples/mudlib -M master.c 4000
telnet localhost 4000    # or: nc localhost 4000
```

## Semantics matched against the original

- driver-hook table: numbers from `mudlib/sys/driver_hook.h`
  (H_NOTIFY_FAIL=10, H_COMMAND=17, H_DEFAULT_PROMPT=21, H_PRINT_PROMPT=22);
  `add_driver_hook`/`query_driver_hook`/`set_prompt` follow `comm.c`
  `print_prompt`/`print_prompt_string` flow.
- `load_object("/x/y")` resolves `/x/y.c`, falls back to the master's
  `compile_object` virtual-object apply (`simulate.c`).
- string handler names (input_to/call_out actions) bind to the setting object
  at set time, like LDMud's bound-lfun semantics.
- `write()` of arrays/mappings renders elements plain; `%O`/`print_string`
  emits LPC literal syntax `({ e })` / `([ k : v ])` matching `sprintf.c`.
- LPC `sscanf`: greedy `%s` with backtracking, `%d`/`%D` rules per
  `doc/efuns/.../sscanf`.

## Tests

```sh
cargo test
```

`tests/driver.rs` runs real LPC through compile→VM and asserts values,
closures/lambdas, sscanf, driver hooks (H_COMMAND), call_outs, and the full
login/command/notify_fail flow; `tests/boot.rs` boots the example mudlib.
