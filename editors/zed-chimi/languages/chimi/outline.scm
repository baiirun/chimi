(function_declaration
  (visibility)? @context
  "fn" @context
  name: (identifier) @name) @item

(const_declaration
  (visibility)? @context
  "const" @context
  name: (identifier) @name) @item

(struct_declaration
  (visibility)? @context
  "struct" @context
  name: (identifier) @name) @item

(interface_declaration
  (visibility)? @context
  "interface" @context
  name: (identifier) @name) @item

(enum_declaration
  (visibility)? @context
  "enum" @context
  name: (identifier) @name) @item

(variant name: (identifier) @name) @item

(field name: (identifier) @name) @item
