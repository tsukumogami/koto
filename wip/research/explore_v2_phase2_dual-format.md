# Dual-Format Configuration Systems Research

## Summary

Research into systems that separate human authoring from machine-readable formats.

### Key Patterns

1. **Terraform (HCL/JSON)**: Both formats are first-class citizens. Either can be authored directly. Same internal representation. Conversion is bidirectional and deterministic.

2. **Protocol Buffers**: Human format (.proto) compiles to binary wire format. One-way compilation. Field numbers provide backward compatibility.

3. **OpenAPI / Kubernetes**: YAML/JSON authoring validated against JSON Schema. Schema is the source of truth. Editor tooling built on schema.

4. **CUE**: Validation rules are part of the data definition itself. Gradual validation -- partial specs are valid. Designed for the "validate-first" philosophy.

5. **Markdoc (Stripe)**: Markdown with custom tags compiles to AST (serializable to JSON). The AST is the intermediate representation -- cacheable, transformable, renderable.

6. **MDX**: Markdown + JSX compiles through multiple stages (mdast -> hast -> estree -> JavaScript). Multi-stage compilation with well-defined intermediate formats.

7. **LLM-assisted validation**: Research shows LLMs useful for suggestions/feedback but core validation must be deterministic schema-based. Temperature tuning and output validation rules address non-determinism.

### Relevance to Koto

Koto templates contain two types of content:
- **Structure** (states, transitions, gates, variables) -- wants a deterministic format
- **Content** (directives -- markdown text the agent reads) -- wants a flexible format

The tension between these creates the design challenge. Most dual-format systems deal with all-structured content (Terraform, Protobuf) or all-content (MDX). Koto needs both.

The most relevant models:
- **Terraform**: proves dual-format authoring with same semantics works
- **Markdoc**: proves markdown can compile to a structured AST
- **OpenAPI/Kubernetes**: proves schema-based validation with editor tooling works
- **CUE**: proves validation-first philosophy is viable for configuration
