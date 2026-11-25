; Zed gives later patterns priority, so general fallbacks come first.

; Capitalized names are types or constructors, by convention.
((identifier) @constructor
  (#match? @constructor "^[A-Z]"))

; Keywords

[
  "import"
  "as"
  "struct"
  "enum"
  "interface"
  "fn"
  "let"
  "const"
  "case"
  "if"
  "loop"
  "break"
  "errors"
  "needs"
  "using"
  "type"
  "native"
  "process"
  "name"
  "test"
] @keyword

[
  "and"
  "or"
  "not"
] @keyword.operator

(visibility) @keyword

; Types

(void_type) @type.builtin
(type_identifier) @type
((type_identifier) @type.builtin
  (#eq? @type.builtin "Never"))
(generic_parameters name: (identifier) @type)
(struct_declaration name: (identifier) @type)
(interface_declaration name: (identifier) @type)
(enum_declaration name: (identifier) @type)
(type_alias_declaration name: (identifier) @type)
(native_type_declaration name: (identifier) @type)
(variant name: (identifier) @constructor)
(named_type module: (identifier) @namespace)
(qualified_type_name module: (identifier) @namespace)
(requirement_group_name module: (identifier) @namespace)

; Modules

(module_path (identifier) @namespace)
(import_declaration alias: (identifier) @namespace)
(import_declaration member: (identifier) @type)

; Functions

(function_declaration name: (identifier) @function)
(native_function_declaration name: (identifier) @function)
(call_expression function: (identifier) @function)
(call_expression
  function: (member_expression member: (identifier) @function))

; Values

(parameter name: (identifier) @variable.parameter)
; A capitalized bare parameter, as in `fn route(Get, path)`, is a clause pattern.
((parameter name: (identifier) @constructor)
  (#match? @constructor "^[A-Z]"))
(let_binding name: (identifier) @variable)
(const_declaration name: (identifier) @constant)
(process_name_declaration name: (identifier) @constant)
(loop_expression state: (identifier) @variable)
(typed_pattern name: (identifier) @variable)
(rest_pattern name: (identifier) @variable)
(alias_pattern alias: (identifier) @variable)
(field name: (identifier) @property)
(record_field name: (identifier) @property)
(field_pattern name: (identifier) @property)
(argument name: (identifier) @property)
(member_expression member: (identifier) @property)

; Literals

(integer) @number
(float) @number
(text "\"" @string)
(text_content) @string
(escape_sequence) @string.escape
(boolean) @boolean
(void_value) @constant.builtin
(comment) @comment

; Operators and punctuation

[
  "+"
  "-"
  "*"
  "/"
  "%"
  "<"
  "<="
  ">"
  ">="
  "=="
  "!="
  "="
  ".."
  "..="
  "|>"
  "->"
  "|"
] @operator

["(" ")" "[" "]" "{" "}"] @punctuation.bracket
["," "." ":"] @punctuation.delimiter

; After the operator and bracket fallbacks, so these win.
(negative_literal "-" @number)
(interpolation
  "${" @punctuation.special
  "}" @punctuation.special)
