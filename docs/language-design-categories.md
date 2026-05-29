# Programming Language Design Categories

This document groups the major components of a programming language and its ecosystem so design questions can be organized before going deep on any one area.

## 1. Core Language

The rules and expressive surface of the language itself.

- Syntax
- Semantics
- Type system
- Data model
- Control flow
- Error model
- Module system
- Metaprogramming

## 2. Runtime

The execution environment and built-in operational behavior.

- Memory management
- Scheduler and concurrency
- Async model
- Process, actor, or service model
- Standard runtime services
- Observability hooks
- Distribution and clustering

## 3. Compiler or Interpreter

The implementation that turns source code into executable behavior.

- Parser
- Type checker
- Optimizer
- Code generator
- Diagnostics
- Incremental compilation
- Target backends, such as native, VM, WASM, BEAM, JavaScript, or LLVM

## 4. Standard Library

The built-in foundation programmers rely on before reaching for packages.

- Collections
- Strings
- Time
- IO
- Networking
- Filesystem
- Serialization
- Cryptography
- Testing primitives
- Concurrency primitives

## 5. Toolchain

The official tools for writing, checking, building, and understanding programs.

- Formatter
- Linter
- Test runner
- Build tool
- Package manager
- Documentation generator
- REPL
- Debugger
- Profiler
- Language server / LSP
- Dependency auditor

## 6. Package Ecosystem

The system for sharing, versioning, and trusting reusable code.

- Package registry
- Versioning rules
- Lockfiles
- Workspace model
- Publishing flow
- Private packages
- Native dependencies
- Supply-chain security

## 7. Developer Experience

The day-to-day experience of writing, reading, debugging, and maintaining code.

- Error messages
- Editor integrations
- Autocomplete
- Jump-to-definition
- Documentation with examples
- Generated API references
- Tracing and debugging views
- Migration tools

## 8. Build and Deployment

How programs become artifacts and run in real environments.

- Artifact format
- Single binary vs runtime bundle
- Cross-compilation
- Containers
- Serverless
- Embedded targets
- Release tooling
- Environment and configuration model

## 9. Interop

How the language communicates with other languages, platforms, and protocols.

- Foreign function interface
- C, Rust, JavaScript, WASM, BEAM, or other runtime interop
- HTTP, gRPC, OpenAPI, or protobuf support
- Foreign error handling
- Foreign memory and resource ownership

## 10. Governance and Compatibility

The social and technical rules that keep the language coherent over time.

- Language specification
- RFC process
- Editions and versions
- Backwards compatibility
- Deprecation policy
- Canonical style guide

## Suggested Question Folders

For design sessions, use these as the main folders:

- Core Language
- Runtime
- Compiler
- Standard Library
- Toolchain
- Packages
- Developer Experience
- Deployment
- Interop
- Governance
