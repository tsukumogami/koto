#!/usr/bin/env bash
# Print the public API of koto::engine::types and koto::error, one item per
# line, sorted, so a change to either surface shows up as a diff against the
# committed snapshot in docs/reference/public-api/.
#
# Usage: scripts/public-api-snapshot.sh [MODULE_PATH...]
#   MODULE_PATH defaults to koto::engine::types and koto::error.
#
# Regenerate and compare the committed snapshot:
#   scripts/public-api-snapshot.sh \
#     > docs/reference/public-api/koto-engine-types-and-error.txt
#   git diff --exit-code docs/reference/public-api/koto-engine-types-and-error.txt
#
# How it works: rustdoc's JSON output (unstable, so RUSTC_BOOTSTRAP=1 lets a
# stable toolchain emit it) is built for koto and for every path dependency
# of koto, then a jq program walks the module tree from each MODULE_PATH,
# following `pub use` re-exports, across crates by name when the target lives
# in a path dependency. Every item is printed under the path a dependent
# names it by, not where it is defined, so moving a module into a crate that
# koto re-exports leaves the output unchanged.
#
# Each line is `<path>: <declaration>`. Structs and enums list their public
# fields and variants, their inherent public methods and associated
# constants, and the traits they implement (blanket and auto-trait impls are
# left out). Attributes rustdoc keeps (serde, error messages, non_exhaustive)
# are shown; lint and inline attributes are not. Types are printed as written
# in the source.
#
# Cargo's output goes to stderr; only the snapshot goes to stdout. The JSON
# format is pinned by its format_version; see the README beside the
# snapshot for the toolchain it was taken with.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

EXPECTED_FORMAT_VERSION=61

if [ "$#" -eq 0 ]; then
    set -- "koto::engine::types" "koto::error"
fi

metadata="$(cargo metadata --locked --no-deps --format-version 1)"
target_dir="$(printf '%s' "$metadata" | jq -r '.target_directory')"
# koto first, then its path dependencies (normal dependencies only).
packages="koto $(printf '%s' "$metadata" | jq -r '
    .packages[] | select(.name == "koto") | .dependencies[]
    | select(.path != null and .kind == null) | .name' | LC_ALL=C sort | tr '\n' ' ')"

json_files=()
for pkg in $packages; do
    RUSTC_BOOTSTRAP=1 cargo rustdoc --locked --lib -p "$pkg" \
        -- -Z unstable-options --output-format json >&2
    file="$target_dir/doc/$(printf '%s' "$pkg" | tr '-' '_').json"
    version="$(jq '.format_version' "$file")"
    if [ "$version" != "$EXPECTED_FORMAT_VERSION" ]; then
        echo "warning: $file has rustdoc JSON format_version $version;" \
            "this script was written against $EXPECTED_FORMAT_VERSION" >&2
    fi
    json_files+=("$file")
done

roots_json="$(printf '%s\n' "$@" | jq -R . | jq -s .)"

# shellcheck disable=SC2016 # $names are jq variables, not shell ones.
jq -r -s --argjson roots "$roots_json" '
. as $docs
| def item($d; $id): $docs[$d].index[$id | tostring];

  # Render a type (and, through the "__args" and "__bound" wrappers, generic
  # arguments and bounds) as Rust source.
  def ty:
    if . == null then "()"
    elif type == "string" then (if . == "infer" then "_" else . end)
    elif has("__args") then
      .__args
      | if . == null then ""
        elif type == "string" then "(..)"
        elif has("angle_bracketed") then
          .angle_bracketed
          | ([.args[] | if type == "string" then "_"
                         elif has("lifetime") then .lifetime
                         elif has("type") then (.type | ty)
                         elif has("const") then (.const.expr)
                         else tojson end]
             + [.constraints[] | .name + ({__args: .args} | ty)
                  + (if .binding.equality then
                       " = " + (.binding.equality | if has("type") then (.type | ty) else (.constant.expr) end)
                     else
                       ": " + ([.binding.constraint[] | {__bound: .} | ty] | join(" + "))
                     end)])
          | if length == 0 then "" else "<" + join(", ") + ">" end
        elif has("parenthesized") then
          .parenthesized
          | "(" + ([.inputs[] | ty] | join(", ")) + ")"
            + (if .output then " -> " + (.output | ty) else "" end)
        else tojson end
    elif has("__bound") then
      .__bound
      | if has("trait_bound") then
          .trait_bound
          | (if .modifier == "maybe" then "?" elif .modifier == "maybe_const" then "~const " else "" end)
            + (if (.generic_params | length) > 0 then
                 "for<" + ([.generic_params[].name] | join(", ")) + "> "
               else "" end)
            + .trait.path + ({__args: .trait.args} | ty)
        elif has("outlives") then .outlives
        elif has("use") then "use<" + ([.use[] | .lifetime // .param // tojson] | join(", ")) + ">"
        else tojson end
    elif has("primitive") then .primitive
    elif has("generic") then .generic
    elif has("resolved_path") then .resolved_path | .path + ({__args: .args} | ty)
    elif has("borrowed_ref") then
      .borrowed_ref
      | "&" + (if .lifetime then .lifetime + " " else "" end)
        + (if .is_mutable then "mut " else "" end) + (.type | ty)
    elif has("raw_pointer") then
      .raw_pointer | "*" + (if .is_mutable then "mut " else "const " end) + (.type | ty)
    elif has("tuple") then "(" + ([.tuple[] | ty] | join(", ")) + ")"
    elif has("slice") then "[" + (.slice | ty) + "]"
    elif has("array") then "[" + (.array.type | ty) + "; " + .array.len + "]"
    elif has("dyn_trait") then
      "dyn " + ([.dyn_trait.traits[] | .trait | .path + ({__args: .args} | ty)]
                + (if .dyn_trait.lifetime then [.dyn_trait.lifetime] else [] end)
                | join(" + "))
    elif has("impl_trait") then "impl " + ([.impl_trait[] | {__bound: .} | ty] | join(" + "))
    elif has("qualified_path") then
      .qualified_path
      | "<" + (.self_type | ty)
        + (if .trait then " as " + .trait.path + ({__args: .trait.args} | ty) else "" end)
        + ">::" + .name + ({__args: .args} | ty)
    elif has("function_pointer") then
      .function_pointer
      | "fn(" + ([.sig.inputs[] | .[1] | ty] | join(", ")) + ")"
        + (if .sig.output then " -> " + (.sig.output | ty) else "" end)
    else tojson end;

  def bounds_of: [.[] | {__bound: .} | ty] | join(" + ");

  # `<...>` for a generics block, skipping the parameters rustdoc
  # synthesizes for `impl Trait` arguments.
  def gparams:
    [.params[]
     | if .kind.lifetime then
         .name + (if (.kind.lifetime.outlives | length) > 0 then ": " + (.kind.lifetime.outlives | join(" + ")) else "" end)
       elif .kind.type then
         if .kind.type.is_synthetic then empty
         else .name
           + (if (.kind.type.bounds | length) > 0 then ": " + (.kind.type.bounds | bounds_of) else "" end)
           + (if .kind.type.default then " = " + (.kind.type.default | ty) else "" end)
         end
       elif .kind.const then
         "const " + .name + ": " + (.kind.const.type | ty)
         + (if .kind.const.default then " = " + .kind.const.default else "" end)
       else tojson end]
    | if length == 0 then "" else "<" + join(", ") + ">" end;

  def wheres:
    [.where_predicates[]
     | if .bound_predicate then
         .bound_predicate | (.type | ty) + ": " + (.bounds | bounds_of)
       elif .lifetime_predicate then
         .lifetime_predicate | .lifetime + ": " + (.outlives | join(" + "))
       elif .eq_predicate then
         .eq_predicate | (.lhs | ty) + " = " + (.rhs | if has("type") then (.type | ty) else (.constant.expr) end)
       else tojson end]
    | if length == 0 then "" else " where " + join(", ") end;

  # Attributes that describe the API or the wire format; lint and codegen
  # hints are left out.
  def attrs:
    [.attrs[]
     | if type == "string" then
         if . == "automatically_derived" then empty else "#[" + . + "]" end
       elif has("other") then
         .other | select((startswith("#[allow(") or startswith("#[expect(")
                          or startswith("#[attr = Inline")) | not)
       else
         "#[" + (to_entries[0] | .key + (if .value == null then "" else " " + (.value | tojson) end)) + "]"
       end]
    | map(. + " ") | join("");

  def fn_decl:
    .header as $h
    | (if $h.is_const then "const " else "" end)
      + (if $h.is_async then "async " else "" end)
      + (if $h.is_unsafe then "unsafe " else "" end)
      + (if $h.abi == "Rust" then "" else "extern " + ($h.abi | tojson) + " " end)
      + "fn" + (.generics | gparams)
      + "(" + ([.sig.inputs[] | .[0] + ": " + (.[1] | ty)] | join(", ")) + ")"
      + (if .sig.output then " -> " + (.sig.output | ty) else "" end)
      + (.generics | wheres);

  # The item `$id` of document `$d`, or the item a re-export names, as a
  # [doc, id] pair. A target outside document `$d` is found by its path in
  # the document of the crate that path starts with.
  def resolve($d; $id):
    def child($d2; $mod; $name):
      [item($d2; $mod).inner.module.items[] | item($d2; .) as $c
       | if $c.inner.use then
           select($c.inner.use.name == $name and $c.visibility == "public")
           | resolve($d2; $c.inner.use.id)
         elif $c.name == $name then [$d2, $c.id]
         else empty end]
      | first // null;
    if $id == null then null
    elif item($d; $id) then [$d, $id]
    else
      ($docs[$d].paths[$id | tostring].path // null) as $path
      | if $path == null then null
        else
          ([range(0; $docs | length) as $i
            | select(item($i; $docs[$i].root).name == $path[0]) | $i] | first // null) as $cd
          | if $cd == null then null
            else reduce $path[1:][] as $seg ([$cd, $docs[$cd].root];
                   if . == null then null else child(.[0]; .[1]; $seg) end)
            end
        end
    end;

  def canon($d; $id):
    ($docs[$d].paths[$id | tostring].path // null) | if . then join("::") else null end;

  def trait_impl_line:
    "impl " + (if .is_negative then "!" else "" end) + .trait.path + ({__args: .trait.args} | ty);

  # Trait impls by the canonical path of the type they are for, from every
  # document, so an impl written in one crate for a type defined in another
  # is still listed with that type.
  ([range(0; $docs | length) as $d2
    | $docs[$d2].index[]
    | select(.inner.impl != null)
    | .inner.impl
    | select(.trait != null and (.is_synthetic | not) and .blanket_impl == null
             and .for.resolved_path != null)
    | {d: $d2, key: canon($d2; .for.resolved_path.id), line: trait_impl_line}
    | select(.key != null)]
   | group_by(.key) | map({key: .[0].key, value: .}) | from_entries) as $trait_impls

  # Lines for the item `$it` of document `$d`, named `$p`. With `$glob`, a
  # module contributes only its members, under `$p` itself.
  | def emit($d; $p; $glob):
    . as $it
    | ($it.inner | keys[0]) as $k
    | ($it.inner[$k]) as $in
    | ($it | attrs) as $a
    | def members:
        $in.items[] | item($d; .) as $c
        | if $c.inner.use then
            $c.inner.use as $u
            | select($c.visibility == "public")
            | resolve($d; $u.id) as $r
            | if $r == null then
                "\($p)::\($u.name // "*"): use \($u.source) (unresolved)"
              elif $u.is_glob then
                item($r[0]; $r[1]) | emit($r[0]; $p; true)
              else
                item($r[0]; $r[1]) | emit($r[0]; "\($p)::\($u.name)"; false)
              end
          elif $c.visibility == "public" then
            $c | emit($d; "\($p)::\($c.name)"; false)
          else empty end;
      # Fields of a struct or variant kind `$fp`, named under `$pp`. A
      # struct lists its public fields only, a variant lists all of them.
      def fields($pp; $fp):
        if $fp == null then empty
        elif $fp.plain then
          ($fp.plain.fields[] | item($d; .) | select(.visibility == "public")
           | "\($pp)::\(.name): \(attrs)field: \(.inner.struct_field | ty)"),
          (if $fp.plain.has_stripped_fields then "\($pp): has private fields" else empty end)
        elif $fp.struct then
          ($fp.struct.fields[] | item($d; .)
           | "\($pp)::\(.name): \(attrs)field: \(.inner.struct_field | ty)"),
          (if $fp.struct.has_stripped_fields then "\($pp): has private fields" else empty end)
        elif $fp.tuple then
          ($fp.tuple | to_entries[]
           | if .value == null then "\($pp)::\(.key): private field"
             else (.key) as $i | item($d; .value)
               | "\($pp)::\($i): \(attrs)field: \(.inner.struct_field | ty)" end)
        else empty end;
      def impls:
        (($in.impls // [])[] | item($d; .) | .inner.impl
         | select((.is_synthetic | not) and .blanket_impl == null)
         | if .trait == null then
             .items[] | item($d; .) | select(.visibility == "public")
             | emit($d; "\($p)::\(.name)"; false)
           else
             "\($p): " + trait_impl_line
           end),
        (canon($d; $it.id) as $key
         | if $key == null then empty
           else ($trait_impls[$key] // [])[] | select(.d != $d) | "\($p): \(.line)" end);
      def kind_word($kind):
        if $kind == null or $kind == "unit" or $kind == "plain" then ""
        elif ($kind | type) == "string" then " " + $kind
        else " " + ($kind | keys[0]) end;
    if $k == "module" then
      (if $glob then empty else "\($p): mod" end), members
    elif $k == "struct" then
      "\($p): \($a)struct\($in.generics | gparams)\(kind_word($in.kind))\($in.generics | wheres)",
      fields($p; $in.kind | if type == "object" then . else null end),
      impls
    elif $k == "union" then
      "\($p): \($a)union\($in.generics | gparams)\($in.generics | wheres)",
      fields($p; {plain: {fields: $in.fields, has_stripped_fields: $in.has_stripped_fields}}),
      impls
    elif $k == "enum" then
      "\($p): \($a)enum\($in.generics | gparams)\($in.generics | wheres)",
      (if $in.has_stripped_variants then "\($p): has private variants" else empty end),
      ($in.variants[] | item($d; .) | . as $v
       | ("\($p)::\($v.name)") as $vp
       | "\($vp): \($v | attrs)variant\(kind_word($v.inner.variant.kind))"
         + (if $v.inner.variant.discriminant then " = " + $v.inner.variant.discriminant.expr else "" end),
         fields($vp; $v.inner.variant.kind | if type == "object" then . else null end)),
      impls
    elif $k == "function" then "\($p): \($a)\($in | fn_decl)"
    elif $k == "constant" then "\($p): \($a)const: \($in.type | ty) = \($in.const.expr)"
    elif $k == "assoc_const" then "\($p): \($a)const: \($in.type | ty)" + (if $in.value then " = " + $in.value else "" end)
    elif $k == "static" then "\($p): \($a)static\(if $in.is_mutable then " mut" else "" end): \($in.type | ty)"
    elif $k == "type_alias" then "\($p): \($a)type\($in.generics | gparams) = \($in.type | ty)\($in.generics | wheres)"
    elif $k == "trait" then
      "\($p): \($a)\(if $in.is_unsafe then "unsafe " else "" end)trait\($in.generics | gparams)"
      + (if ($in.bounds | length) > 0 then ": " + ($in.bounds | bounds_of) else "" end)
      + ($in.generics | wheres),
      ($in.items[] | item($d; .) | emit($d; "\($p)::\(.name)"; false))
    elif $k == "assoc_type" then
      "\($p): \($a)type" + (if ($in.bounds | length) > 0 then ": " + ($in.bounds | bounds_of) else "" end)
      + (if $in.type then " = " + ($in.type | ty) else "" end)
    else "\($p): \($a)\($k)" end;

  $roots[] as $root
  | ($root | split("::")) as $path
  | ([range(0; $docs | length) as $i
      | select(item($i; $docs[$i].root).name == $path[0]) | $i] | first // null) as $cd
  | if $cd == null then error("no crate named \($path[0]) among the documents")
    else
      (reduce $path[1:][] as $seg ([$cd, $docs[$cd].root];
         if . == null then null
         else . as [$dd, $mm]
           | [item($dd; $mm).inner.module.items[] | item($dd; .) as $c
              | if $c.inner.use then
                  select($c.inner.use.name == $seg and $c.visibility == "public")
                  | resolve($dd; $c.inner.use.id)
                elif $c.name == $seg then [$dd, $c.id]
                else empty end]
           | first // null
         end)) as $r
      | if $r == null then error("\($root) not found")
        else item($r[0]; $r[1]) | emit($r[0]; $root; false) end
    end
' "${json_files[@]}" | LC_ALL=C sort -u
