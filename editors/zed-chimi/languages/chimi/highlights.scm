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
  "case"
  "if"
  "loop"
  "break"
  "errors"
  "needs"
  "using"
] @keyword

(visibility) @keyword

; Types

(void_type) @type.builtin
(type_identifier) @type
(generic_parameters name: (identifier) @type)
(struct_declaration name: (identifier) @type)
(interface_declaration name: (identifier) @type)
(enum_declaration name: (identifier) @type)
(variant name: (identifier) @constructor)
(named_type module: (identifier) @namespace)

; Modules

(module_path (identifier) @namespace)
(import_declaration alias: (identifier) @namespace)
(import_declaration member: (identifier) @type)

; Functions

(function_declaration name: (identifier) @function)
(call_expression function: (identifier) @function)
(call_expression
  function: (member_expression member: (identifier) @function))

; Values

(parameter name: (identifier) @variable.parameter)
(let_binding name: (identifier) @variable)
(loop_expression state: (identifier) @variable)
(field name: (identifier) @property)
(record_field name: (identifier) @property)
(argument name: (identifier) @property)
(member_expression member: (identifier) @property)

; Literals

(integer) @number
(text) @string
(escape_sequence) @string.escape
(boolean) @boolean
(comment) @comment

; Operators and punctuation

[
  "+"
  "-"
  "*"
  "/"
  "<"
  "<="
  ">"
  ">="
  "="
  "|>"
  "->"
] @operator

["(" ")" "[" "]" "{" "}"] @punctuation.bracket
["," "." ":"] @punctuation.delimiter
