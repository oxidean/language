# Methods and interface dispatch (D3)

Methods are fns declared on a named type; interface-typed values are
fat `{data, vtable}` pairs dispatched Go-style. Deliberately shallow —
structural satisfaction, no trait solving, no inheritance.

```oxl
struct Point { x: int, y: int }

fn (p Point) norm() -> int { return p.x * p.x + p.y * p.y }
fn (p mut Point) shift(dx: int, dy: int) {
    p.x = p.x + dx
    p.y = p.y + dy
}

p := Point{x: 3, y: 4}
assert(p.norm() == 25)   // receiver sugar
assert(norm(p) == 25)    // same call, free-fn shape
p.shift(1, 1)            // mut receiver writes through
```

## Declaring

`fn (r T) name(params)` — the receiver sits before the method name and
behaves as implicit first parameter. `r T`, `r own T`, `r ref T`, and
`r mut T` receivers are all allowed (`fn (r: T)` parses too). The
receiver base must be a declared struct or enum — no methods on
primitives, slices, or fn types.

The method's *declared name* is `T.name` after module qualification —
two types may share a method name, and `norm(p)` (receiver in arg
position) calls the same fn as `p.norm()`.

Receiver adaptation mirrors Go: `p.shift(..)` on a value `p` passes
`&p` (requires an lvalue — `f().shift(..)` is an error, bind first);
`(&p).norm()`-style pointer receivers dereference for a value receiver.

## Interfaces as types

`interface` bodies now accept method decls alongside the op vocabulary:

```oxl
interface Writer { write(s: str) -> int }

struct Upper { n: int }
fn (u Upper) write(s: str) -> int { return u.n + len(s) }

let w: Writer = Upper{n: 10}   // structural — no `implements`
assert(w.write("abc") == 13)   // vtable dispatch
```

Satisfaction is structural: a type satisfies an interface iff a
non-generic `T.name` fn exists for each declared method with matching
params (after the receiver) and return. Interfaces with only ops keep
their old role as generic constraints; interfaces with methods are
additionally *types*.

An interface value is an `RtIface { void *data, void **vtab }` pair.
Value impls are boxed so the data outlives the stack slot (mut
receivers write through the box); pointer impls (`own`/`ref`/`mut`)
hand the pointer over directly. `w == nil` compares the data pointer;
comparing two non-nil ifaces is a compile error.

Interface-to-interface assignment rewraps when the source covers the
target (the thunk forwards through the source vtab). `nil` assigns to
any interface.

## Generics

A method-declaring interface works as a constraint — `fn f[T Writer](v: T)`
may call `v.write(..)`, monomorphizing to the concrete `T.write` per
instantiation. A generic *impl* (`fn (s Stack[T]) push[..]`) satisfies
interfaces too: the vtab slot pins the instantiation the interface's
declared signature implies.

## Limits (v0.2)

- **Methods may not suspend** — `spawn`/`select`/`recv`/`send`/`sleep`
  inside a method body is a compile error; vtable calls carry no task
  frame.
- No associated types, no interface generics, no embedding; structs
  aren't generic, so only *method* ty params pin through a vtab slot.
- A fn-valued struct field `x.m` shadows a method `m` — `x.m()` calls
  the field.
- The bootstrap codegen (`bootstrap/codegen.oxl`) does not lower
  method dispatch — parser and checker handle `fn (r T) m` receivers
  and interface sigs; lowering stays on the Rust pipeline.
