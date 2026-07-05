## Commit Messages
Use **Conventional Commits** with **short subject line only**. No body, no footers, no explanations.
**Good examples:**
- `fix(wallet): refresh JWT after setup completes`
- `feat(wallet): cut over to type-3 deposit wallet contract`
- `refactor(commands): simplify error mapping for deposits`
- `chore(tauri): update capability for filesystem access`

**Never** write long commit bodies or "This change does X because...".

## Core Philosophy — Write Code That Looks Human
- **Match existing code 1:1** — Before touching any file, read the surrounding functions and module. Copy naming, style, error patterns, and level of abstraction exactly. Do not "improve" the local style.
- **Minimal viable solution** — Solve the task with the smallest amount of code possible. No extra abstractions, traits, newtypes, builder patterns, or "future-proofing" unless there is a clear, immediate reduction in duplication or readability win *in this exact context*.
- **Avoid churny refactors** — Never introduce a one-use temporary variable just to name it before passing it on. Keep the expression inline unless the name carries real domain meaning or eliminates real duplication.
- **Be slightly lazy where it makes sense** — If the surrounding code is concise and works, match it. Don't add ceremony that isn't already present.
- **Comments**:
  - Never comment what the code does (code should be self-documenting).
  - Only comment *why* — a non-obvious business reason, important trade-off, or invariant the type system doesn't enforce.
  - No emojis. No corporate fluff ("leverage", "utilize", "robustly", "seamlessly", "best practice", etc.).
- **After writing code** — Re-read the changed functions. Remove any defensive checks, clones, or unwraps that are now guaranteed safe by the caller or surrounding invariants.
- **Naming** — Use the short, domain-specific names already established in the project (e.g. `tx`, `addr`, `bal`, `wallet`). Never default to generic names like `data`, `result`, `item`, `processed`, `response`, `value`, `obj`, `entity`, etc.

## Rust Idioms & Anti-AI Slop Rules (Strict)
These rules eliminate the patterns Rust developers immediately recognize as LLM-generated.

### Ownership & Borrowing
- Prefer `&T` / `&mut T` borrowing over moving or cloning whenever the caller can provide a reference.
- Avoid gratuitous `.clone()` (especially on `String`, `Vec`, `PathBuf`, `Arc`). Only clone when ownership transfer is required.
- Never hold a `MutexGuard` (or equivalent) across an `.await`. Drop it first or use the right async mutex.

### Error Handling (Very Important for Tauri)
- Use the `?` operator aggressively.
- Domain/library errors → `thiserror::Error` enum + `#[from]`.
- Tauri commands (exposed to frontend) → return `Result<T, String>` or a small serializable error type. Map at the command boundary; do **not** leak internal details to JS unless intentional.
- **Never** use `.unwrap()` or `.expect(...)` in command handlers, hot paths, or production logic. Use `?` or explicit `map_err`.
- `.expect("invariant violated: ...")` is only acceptable for true startup invariants in `main`/`setup` with a clear message.

### Tauri Commands & State
- Always mark with `#[tauri::command]`.
- Prefer `async fn` for anything involving I/O, network, crypto, or potentially slow work.
- Access shared state with `tauri::State<'_, MyState>`.
- Mutable state inside `State` must be wrapped in `Mutex` / `RwLock` (or `parking_lot`). Lock for the shortest possible duration.
- Keep command functions thin — delegate real work to domain functions.

### Control Flow & Functions
- Small focused functions + early returns / `?` to reduce nesting.
- Prefer `if let`, `while let`, and idiomatic `match` over verbose alternatives.
- Use iterators + combinators (`.map()`, `.filter()`, `.fold()`, etc.) when they are natural — not when they make the code harder to read than a simple loop.
- No `todo!()`, `unimplemented!()`, or `panic!` as control flow in shipped code.

### Types & Structs
- Private fields by default.
- Derive only what is actually used (`Debug`, `Clone`, `Serialize`, `Deserialize`, `PartialEq`, `Default`, etc.).
- Newtypes only when they provide real type safety.
- Avoid deeply nested `Arc<Mutex<Arc<...>>>` — keep state modeling simple and idiomatic.

### Formatting, Lints & Imports
- Code you produce **must** be `cargo fmt` clean and pass `cargo clippy -- -D warnings` (treat warnings as errors).
- Imports: std → external crates → local (rustfmt handles ordering). No wildcards except in `#[cfg(test)]` modules or preludes.
- After any edit, the code should be ready to commit cleanly.

## What NOT to Do (Common AI Markers to Avoid)
- Long doc comments on private items explaining the obvious.
- "TODO: improve later" or essay-style comments.
- Introducing extra `Result` wrapper types or error variants "just in case".
- Renaming variables to "more descriptive" names when the project already uses short domain names.
- Adding unnecessary derives, traits, or abstractions.
- Turning simple `if` into `match` "for clarity".
- Over-defensive programming or redundant checks.
- Assuming a design pattern should be introduced.

when you can use subagnets, use them, use subagents to save your context

## Workflow (Every Task)
1. Read this `AGENTS.md` + the exact files/modules you will change.
2. Match the local style, naming, and error handling perfectly.
3. Make the smallest possible change.
4. After editing: ensure `cargo fmt` + `cargo clippy` would pass.
5. Strip any now-redundant defensive code.
6. Use a short conventional commit message.
