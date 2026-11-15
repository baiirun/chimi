"""Mutants of the lane-fixes bugs: closure captures of pattern-parameter
literals (ts-270757), the occurs check in unification (ts-1bba2b), and native
Text that keeps its bytes after U+0000 (ts-ce73fb).

Run: scripts/mutation/mutate.py scripts/mutation/lane_fixes_mutations.py
Each mutant must turn the named language or cli test red.
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
NUL = "text_holding_nul_keeps_every_byte_on_both_backends"


def language(name, file, edits, filter):
    return dict(name=name, file=file, edits=edits, targets=["language"], filter=filter)


def cli(name, file, edits, filter):
    return dict(name=name, file=file, edits=edits, targets=["cli"], filter=filter)


def nul(name, edits):
    return language(name, RUNTIME, edits, NUL)


MUTATIONS = [
    language("captures are computed before the literal's aliases", "src/executable.rs", [(
        "                let lowered_body = self.expression(module_id, body);\n"
        "                CoreOperation::Closure {\n"
        "                    parameters,\n"
        "                    captures: self.closure_captures(module_id, body),\n",
        "                let captures = self.closure_captures(module_id, body);\n"
        "                let lowered_body = self.expression(module_id, body);\n"
        "                CoreOperation::Closure {\n"
        "                    parameters,\n"
        "                    captures,\n")], "a_function_literal_with_pattern_parameters"),
    language("unification has no occurs check", "src/typing.rs", [(
        "        if self.inference_variable_occurs(variable, type_id) {",
        "        if false {")], "a_type_that_must_contain_itself"),
    language("the occurs check skips type arguments", "src/typing.rs", [(
        "            CanonicalType::Named { arguments, .. } => arguments.iter().copied().any(occurs),",
        "            CanonicalType::Named { .. } => false,")], "a_type_that_must_contain_itself"),
    cli("the spread fixture still overflows", "src/typing.rs", [(
        "        if self.inference_variable_occurs(variable, type_id) {",
        "        if false {")], "invalid_programs_stop_at_the_owning_stage"),
    nul("literals stop at U+0000", [(
        "static ChimiValue *string_text(uint32_t id) { return text_from_bytes(CHIMI_STRINGS[id], CHIMI_STRING_LENGTHS[id]); }",
        "static ChimiValue *string_text(uint32_t id) { return text_value(CHIMI_STRINGS[id]); }")]),
    nul("text patterns compare as C strings", [(
        "text_equals(value, CHIMI_STRINGS[pattern->offset], CHIMI_STRING_LENGTHS[pattern->offset]);",
        "strcmp(value->as.text.bytes, CHIMI_STRINGS[pattern->offset]) == 0;")]),
    nul("equal prefixes compare equal", [(
        "        order = bytes ? (bytes > 0) - (bytes < 0) : order_of(left_length, right_length); break;",
        "        order = (bytes > 0) - (bytes < 0); break;")]),
    nul("Text.equal compares as C strings", [(
        "      return success(boolean_value(text_equals(arguments[0], arguments[1]->as.text.bytes, arguments[1]->as.text.length)));",
        "      return success(boolean_value(strcmp(arguments[0]->as.text.bytes, arguments[1]->as.text.bytes) == 0));")]),
    nul("contains stops at U+0000", [(
        "      return success(boolean_value(text_find(arguments[0]->as.text.bytes, arguments[0]->as.text.length,\n"
        "                                             arguments[1]->as.text.bytes, arguments[1]->as.text.length) != NULL));",
        "      return success(boolean_value(strstr(arguments[0]->as.text.bytes, arguments[1]->as.text.bytes) != NULL));")]),
    nul("split's last part stops at U+0000", [(
        "      return success(list_push(parts, text_from_bytes(text, (size_t)(end - text))));",
        "      return success(list_push(parts, text_value(text)));")]),
    nul("replace's tail stops at U+0000", [(
        "      buffer_bytes(&buffer, text, (size_t)(end - text));\n"
        "      return success(text_from_bytes(buffer.data ? buffer.data : \"\", buffer.length));",
        "      buffer_bytes(&buffer, text, strlen(text));\n"
        "      return success(text_from_bytes(buffer.data ? buffer.data : \"\", buffer.length));")]),
    nul("concat stops each part at U+0000", [(
        "        buffer_bytes(&buffer, part->as.text.bytes, part->as.text.length);",
        "        buffer_bytes(&buffer, part->as.text.bytes, strlen(part->as.text.bytes));")]),
    nul("byte_length stops at U+0000", [(
        "      return success(integer_value((int64_t)arguments[0]->as.text.length));",
        "      return success(integer_value((int64_t)strlen(arguments[0]->as.text.bytes)));")]),
    nul("trim_end starts at U+0000", [(
        "      size_t end = arguments[0]->as.text.length, width;",
        "      size_t end = strlen(text), width;")]),
    nul("outcome JSON stops at U+0000", [(
        "      case VALUE_TEXT: json_bytes(output, value->as.text.bytes, value->as.text.length); break;",
        "      case VALUE_TEXT: json_string(output, value->as.text.bytes); break;")]),
    nul("Json.encode stops at U+0000", [(
        "  for (const unsigned char *end = cursor + text->as.text.length; cursor < end; ++cursor) {\n"
        "    switch (*cursor) {\n"
        "      case '\"': buffer_bytes(buffer, \"\\\\\\\"\", 2); break;",
        "  for (; *cursor; ++cursor) {\n"
        "    switch (*cursor) {\n"
        "      case '\"': buffer_bytes(buffer, \"\\\\\\\"\", 2); break;")]),
    nul("Debug.render stops at U+0000", [(
        "  for (const unsigned char *end = cursor + text->as.text.length; cursor < end; ++cursor) {\n"
        "    switch (*cursor) {\n"
        "      case '\"': debug_text(buffer, \"\\\\\\\"\"); break;",
        "  for (const unsigned char *end = cursor + strlen((const char *)cursor); cursor < end; ++cursor) {\n"
        "    switch (*cursor) {\n"
        "      case '\"': debug_text(buffer, \"\\\\\\\"\"); break;")]),
    nul("Json.decode stops a string at U+0000", [(
        "    if (value == '\"') return text_from_bytes(buffer.data ? buffer.data : \"\", buffer.length);",
        "    if (value == '\"') return text_value(buffer.data ? buffer.data : \"\");")]),
    nul("Duration.parse reads a C string", [(
        "    int64_t value = 0; uint32_t error = text_holds_nul(arguments[0]) ? CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID",
        "    int64_t value = 0; uint32_t error = false ? CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID")]),
    language("Int.parse reads a C string", RUNTIME, [(
        "      int64_t value = 0; uint32_t error = text_holds_nul(arguments[0]) ? CHIMI_CONSTRUCTOR_INT_PARSE_INVALID_DIGIT",
        "      int64_t value = 0; uint32_t error = false ? CHIMI_CONSTRUCTOR_INT_PARSE_INVALID_DIGIT")],
        "int_parse_accepts_decimal_text_and_names_each_failure"),
    language("Float.parse reads a C string", RUNTIME, [(
        "      double value = 0; uint32_t error = text_holds_nul(arguments[0]) ? CHIMI_CONSTRUCTOR_FLOAT_PARSE_INVALID",
        "      double value = 0; uint32_t error = false ? CHIMI_CONSTRUCTOR_FLOAT_PARSE_INVALID")],
        "float_parse_rejects_text_outside_the_finite_literal_grammar"),
    cli("a path holding U+0000 opens its prefix", RUNTIME, [(
        "  if (text_holds_nul(arguments[0]))\n"
        "    return failure(standard_value(CHIMI_CONSTRUCTOR_FILE_OPEN,",
        "  if (false)\n"
        "    return failure(standard_value(CHIMI_CONSTRUCTOR_FILE_OPEN,")],
        "native_host_io_keeps_text_bytes_after_nul"),
    cli("Console stops at U+0000", RUNTIME, [(
        "  size_t length = arguments[0]->as.text.length;\n"
        "  if (fwrite(",
        "  size_t length = strlen(arguments[0]->as.text.bytes);\n"
        "  if (fwrite(")],
        "native_host_io_keeps_text_bytes_after_nul"),
]
