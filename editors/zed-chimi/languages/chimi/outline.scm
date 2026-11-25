(function_declaration
  (visibility)? @context
  "fn" @context
  name: (identifier) @name) @item

(native_function_declaration
  (visibility)? @context
  "native" @context
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

(struct_declaration (field_list (field name: (identifier) @name) @item))

(interface_declaration (field_list (field name: (identifier) @name) @item))

(type_alias_declaration
  (visibility)? @context
  "type" @context
  name: (identifier) @name) @item

(native_type_declaration
  (visibility)? @context
  "native" @context
  "type" @context
  name: (identifier) @name) @item

(needs_group_declaration
  (visibility)? @context
  "needs" @context
  name: (type_identifier) @name) @item

(process_name_declaration
  (visibility)? @context
  "process" @context
  "name" @context
  name: (identifier) @name) @item

(test_declaration
  "test" @context
  name: (text) @name) @item
