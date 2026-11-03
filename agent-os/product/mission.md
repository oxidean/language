# Product Mission

## Problem

Modern systems languages force a trade: Go delivers best-in-class DX and
instant builds but carries a tracing GC and heavyweight runtime; Rust-class
languages deliver lean memory and native performance at the cost of
borrow-checker friction and slow builds. There is no good option for teams
that want Go ergonomics *and* a lean native footprint.

## Target Users

Initially: internal oxidean projects and services — the language's first
real users, via the pilot apps. Almost immediately after: the open-source
public — developers who want Go-like productivity for lean CLI tools,
services, and eventually GUI and web apps.

## Solution

A statically-typed language with Go-shaped DX on a lean runtime: dynamic
ownership/borrow checking instead of a tracing GC or a static borrow
checker, structured concurrency on stackless coroutines with an opt-in
executor, Go-style errors and generics, AOT native compilation with fast
builds, and one integrated toolchain binary.
