/**
 * Tree-sitter grammar for Chimi, used for editor highlighting and structure.
 *
 * The compiler's lossless parser in `compiler/chimi/src/syntax.rs` is the
 * authority. This grammar is deliberately more permissive: line breaks are
 * whitespace here, so a few constructs that span lines parse differently than
 * in the compiler. That only affects highlighting of code the compiler already
 * reports. A call's `(` must follow its callee with no space, which is how
 * `(` on the next line still starts a new case arm or block item.
 *
 * scripts/check-grammar.sh, run in CI, checks that every .chimi file the
 * compiler parses also parses here.
 */

// Loosest to tightest, matching the operator table in
// docs/specification/core-data-and-operators.md.
const PREC = {
  using: 1,
  or: 2,
  and: 3,
  not: 4,
  pipeline: 5,
  comparison: 6,
  range: 7,
  additive: 8,
  multiplicative: 9,
  unary: 10,
  postfix: 11,
};

const commaSep1 = (rule) => seq(rule, repeat(seq(',', rule)), optional(','));
const commaSep = (rule) => optional(commaSep1(rule));

module.exports = grammar({
  name: 'chimi',

  extras: ($) => [/\s/, $.comment],

  word: ($) => $.identifier,

  supertypes: ($) => [$._expression, $._type, $._pattern],

  // The compiler ends an expression at a line break; this grammar cannot see
  // line breaks, so a case arm's body and a bare `break` can be read as
  // continuing onto the next arm, as in `(0, code) -> code` followed by
  // `-2 -> other`. These names are left unordered against the operators, so
  // the generator keeps both readings (the conflicts below) and the reading
  // that fails on the next `->` is dropped.
  precedences: (_) => [['case_arm'], ['break_value']],

  // The two struct conflicts keep a name both as a value and as the start of
  // a struct literal until the next token decides, which is how
  // `case subject {` keeps its arms.
  conflicts: ($) => [
    [$.break_expression],
    [$.binary_expression, $.case_arm],
    [$._expression, $.struct_expression],
    [$._expression, $.qualified_type_name],
  ],

  rules: {
    source_file: ($) => repeat($._item),

    _item: ($) =>
      choice(
        $.import_declaration,
        $.struct_declaration,
        $.enum_declaration,
        $.interface_declaration,
        $.type_alias_declaration,
        $.native_type_declaration,
        $.needs_group_declaration,
        $.process_name_declaration,
        $.function_declaration,
        $.native_function_declaration,
        $.const_declaration,
        $.test_declaration,
        $.let_binding,
        $._expression,
      ),

    // Declarations

    import_declaration: ($) =>
      seq(
        'import',
        field('path', $.module_path),
        choice(
          seq('as', field('alias', $.identifier), optional($._import_members)),
          $._import_members,
        ),
      ),

    // `import std/clock as Clock.{Clock, Instant}` binds the alias and the
    // members at once.
    _import_members: ($) => seq('.', '{', commaSep1(field('member', $.identifier)), '}'),

    module_path: ($) => seq($.identifier, repeat(seq('/', $.identifier))),

    visibility: (_) => 'pub',

    struct_declaration: ($) =>
      seq(
        optional($.visibility),
        'struct',
        field('name', $.identifier),
        optional($.generic_parameters),
        $.field_list,
      ),

    interface_declaration: ($) =>
      seq(
        optional($.visibility),
        'interface',
        field('name', $.identifier),
        optional($.generic_parameters),
        $.field_list,
      ),

    field_list: ($) => seq('{', repeat(seq($.field, optional(','))), '}'),

    field: ($) => seq(field('name', $.identifier), ':', field('type', $._type)),

    enum_declaration: ($) =>
      seq(
        optional($.visibility),
        'enum',
        field('name', $.identifier),
        optional($.generic_parameters),
        '{',
        repeat(seq($.variant, optional(','))),
        '}',
      ),

    variant: ($) =>
      seq(
        field('name', $.identifier),
        optional(seq('(', commaSep($.field), ')')),
      ),

    // `type`, `native`, `process`, `name` and `test` are contextual words in the
    // compiler. Tree-sitter's keyword extraction reads each as a keyword only
    // where the grammar allows it, so `type` stays an ordinary name elsewhere.
    type_alias_declaration: ($) =>
      seq(
        optional($.visibility),
        'type',
        field('name', $.identifier),
        optional($.generic_parameters),
        '=',
        field('type', $._type),
      ),

    native_type_declaration: ($) =>
      seq(
        optional($.visibility),
        'native',
        'type',
        field('name', $.identifier),
        optional($.generic_parameters),
      ),

    // `pub needs Store { db: Database, clock: Clock }` names a group of needs.
    needs_group_declaration: ($) =>
      seq(
        optional($.visibility),
        'needs',
        field('name', alias($.identifier, $.type_identifier)),
        $.requirement_record,
      ),

    process_name_declaration: ($) =>
      seq(
        optional($.visibility),
        'process',
        'name',
        field('name', $.identifier),
        ':',
        field('message', $._type),
      ),

    test_declaration: ($) =>
      seq(optional($.visibility), 'test', field('name', $.text), field('body', $.block)),

    generic_parameters: ($) =>
      seq('<', commaSep1(field('name', $.identifier)), '>'),

    // Adjacent declarations with one name are the clauses of one function. A
    // clause's guard comes after the contract and right before the body.
    function_declaration: ($) =>
      seq(
        optional($.visibility),
        'fn',
        field('name', $.identifier),
        optional($.generic_parameters),
        field('parameters', $.parameters),
        optional(field('return_type', $.return_type)),
        optional($.requirement_clause),
        optional(seq('if', field('guard', $._expression))),
        field('body', $.block),
      ),

    // A native function has no body; only a native function declares an
    // `errors` row, since ordinary functions' failures are inferred.
    native_function_declaration: ($) =>
      prec.right(
        seq(
          optional($.visibility),
          'native',
          'fn',
          field('name', $.identifier),
          optional($.generic_parameters),
          field('parameters', $.parameters),
          optional(field('return_type', $.return_type)),
          optional($.errors_clause),
          optional($.requirement_clause),
        ),
      ),

    errors_clause: ($) => prec.right(seq('errors', field('errors', $._type))),

    const_declaration: ($) =>
      seq(
        optional($.visibility),
        'const',
        field('name', $.identifier),
        optional(seq(':', field('type', $._type))),
        '=',
        field('value', $._expression),
      ),

    parameters: ($) => seq('(', commaSep($.parameter), ')'),

    // A parameter is a pattern, as in `fn area(Circle(radius))`. The plain
    // `name` and `name: Type` forms keep their own fields.
    parameter: ($) => choice(plainBinding($), field('pattern', $._pattern)),

    return_type: ($) => seq('->', $._type),

    requirement_clause: ($) =>
      prec.right(
        seq(
          'needs',
          $._requirement,
          repeat(seq('+', $._requirement)),
        ),
      ),

    _requirement: ($) =>
      choice(
        alias($.identifier, $.type_identifier),
        $.requirement_group_name,
        $.requirement_record,
      ),

    // `Store.Needs` names another module's needs group.
    requirement_group_name: ($) =>
      seq(field('module', $.identifier), '.', field('name', alias($.identifier, $.type_identifier))),

    requirement_record: ($) => seq('{', commaSep1($.field), '}'),

    // Types

    _type: ($) =>
      choice(
        $.named_type,
        $.function_type,
        $.tuple_type,
        $.record_type,
        $.union_type,
        $.void_type,
      ),

    // `{ name: Text, age: Int }`, as in a type alias.
    record_type: ($) => seq('{', repeat1(seq($.field, optional(','))), '}'),

    // `HttpError | JobError`: a failure set.
    union_type: ($) => prec.left(1, seq($._type, '|', $._type)),

    void_type: (_) => 'Void',

    named_type: ($) =>
      prec.right(
        seq(
          repeat(seq(field('module', $.identifier), '.')),
          field('name', alias($.identifier, $.type_identifier)),
          optional($.type_arguments),
        ),
      ),

    type_arguments: ($) => seq('<', commaSep1($._type), '>'),

    // Two or more elements; `(T)` and `()` are not tuple types.
    tuple_type: ($) => seq('(', $._type, repeat1(seq(',', $._type)), optional(','), ')'),

    function_type: ($) =>
      prec.right(
        seq(
          'fn',
          optional($.generic_parameters),
          '(',
          commaSep($._type),
          ')',
          '->',
          field('return_type', $._type),
          optional($.errors_clause),
          optional($.requirement_clause),
        ),
      ),

    // Bindings and expressions

    // `let` takes an irrefutable pattern, such as `(low, high)`.
    let_binding: ($) =>
      seq(
        'let',
        choice(plainBinding($), field('pattern', $._pattern)),
        '=',
        field('value', $._expression),
      ),

    _expression: ($) =>
      choice(
        $.using_expression,
        $.pipeline_expression,
        $.binary_expression,
        $.unary_expression,
        $.range_expression,
        $.call_expression,
        $.member_expression,
        $.integer,
        $.float,
        $.text,
        $.boolean,
        $.void_value,
        $.identifier,
        $.parenthesized_expression,
        $.tuple_expression,
        $.list_expression,
        $.record_expression,
        $.struct_expression,
        $.block,
        $.case_expression,
        $.loop_expression,
        $.break_expression,
        $.function_expression,
      ),

    using_expression: ($) =>
      prec.left(
        PREC.using,
        seq(field('operation', $._expression), 'using', field('providers', $._expression)),
      ),

    pipeline_expression: ($) =>
      prec.left(
        PREC.pipeline,
        seq(field('value', $._expression), '|>', field('transform', $._expression)),
      ),

    // Comparisons do not chain in the compiler (`S1006`); this grammar accepts
    // a chain so the rest of the line still highlights.
    binary_expression: ($) => {
      const table = [
        [PREC.or, 'or'],
        [PREC.and, 'and'],
        [PREC.comparison, choice('==', '!=', '<', '<=', '>', '>=')],
        [PREC.additive, choice('+', '-')],
        [PREC.multiplicative, choice('*', '/', '%')],
      ];
      return choice(
        ...table.map(([precedence, operator]) =>
          prec.left(
            precedence,
            seq(
              field('left', $._expression),
              field('operator', operator),
              field('right', $._expression),
            ),
          ),
        ),
      );
    },

    // `not` binds more loosely than `|>` and comparison: `not a == b` is
    // `not (a == b)`.
    unary_expression: ($) =>
      choice(
        prec(PREC.unary, seq(field('operator', '-'), field('operand', $._expression))),
        prec(PREC.not, seq(field('operator', 'not'), field('operand', $._expression))),
      ),

    // `start..end` is half-open and `start..=end` inclusive. Like comparisons,
    // a chain is accepted here and rejected by the compiler.
    range_expression: ($) =>
      prec.left(
        PREC.range,
        seq(
          field('start', $._expression),
          field('operator', choice('..', '..=')),
          field('end', $._expression),
        ),
      ),

    call_expression: ($) =>
      prec(PREC.postfix, seq(field('function', $._expression), field('arguments', $.arguments))),

    // The `(` of a call follows its callee directly. A line break ends an
    // expression in the compiler, so `(` on the next line starts a new item,
    // such as a case arm's tuple pattern; whitespace is the nearest test here.
    arguments: ($) => seq(token.immediate('('), commaSep($.argument), ')'),

    argument: ($) =>
      seq(optional(seq(field('name', $.identifier), ':')), field('value', $._expression)),

    member_expression: ($) =>
      prec(
        PREC.postfix,
        seq(field('object', $._expression), '.', field('member', $.identifier)),
      ),

    parenthesized_expression: ($) => seq('(', $._expression, ')'),

    // Two or more elements; `(e)` is grouping, and `(e,)` and `()` are errors.
    tuple_expression: ($) =>
      seq('(', $._expression, repeat1(seq(',', $._expression)), optional(','), ')'),

    list_expression: ($) => seq('[', commaSep($._expression), ']'),

    // A record update puts `..base` first: `{ ..config, jobs: jobs }`.
    record_expression: ($) =>
      seq(
        '{',
        choice(
          seq($.record_spread, repeat(seq(',', $.record_field)), optional(',')),
          commaSep1($.record_field),
        ),
        '}',
      ),

    // `Point { x: 1, y: 2 }`, `Config { ..defaults, jobs: 4 }` or `Marker {}`:
    // a struct built by field name. Calls and enum variants stay positional.
    // The name is read as a type only while braces with fields can follow, so
    // `case subject {` keeps its arms: the struct reading fails at the first
    // arm and the parser drops it.
    struct_expression: ($) =>
      seq(
        field('name', choice(alias($.identifier, $.type_identifier), $.qualified_type_name)),
        field('fields', $.struct_fields),
      ),

    qualified_type_name: ($) =>
      seq(field('module', $.identifier), '.', field('name', alias($.identifier, $.type_identifier))),

    struct_fields: ($) =>
      seq(
        '{',
        optional(
          choice(
            seq($.record_spread, repeat(seq(',', $.record_field)), optional(',')),
            commaSep1($.record_field),
          ),
        ),
        '}',
      ),

    record_spread: ($) => seq('..', field('base', $._expression)),

    record_field: ($) =>
      seq(field('name', $.identifier), ':', field('value', $._expression)),

    block: ($) => seq('{', repeat1(choice($.let_binding, $._expression)), '}'),

    case_expression: ($) =>
      seq(
        'case',
        field('subject', $._expression),
        repeat(seq(',', field('subject', $._expression))),
        '{',
        repeat($.case_arm),
        '}',
      ),

    case_arm: ($) =>
      prec(
        'case_arm',
        seq(
          commaSep1(field('pattern', $._pattern)),
          optional(seq('if', field('guard', $._expression))),
          '->',
          field('body', $._expression),
        ),
      ),

    // Patterns

    _pattern: ($) =>
      choice(
        $.integer,
        $.float,
        $.negative_literal,
        $.text,
        $.boolean,
        $.void_value,
        $.constructor_pattern,
        $.struct_pattern,
        $.typed_pattern,
        $.tuple_pattern,
        $.list_pattern,
        $.alias_pattern,
      ),

    // A name, a wildcard `_`, or a variant or struct with positional payload
    // patterns, such as `Value.Text(schema)`.
    constructor_pattern: ($) =>
      prec.right(
        seq(
          sep1($.identifier, '.'),
          optional(seq('(', commaSep($._pattern), ')')),
        ),
      ),

    // `Point { x: 0, .. }` matches a struct's fields by name; `..` ignores the rest.
    struct_pattern: ($) =>
      seq(
        sep1($.identifier, '.'),
        '{',
        commaSep(choice($.field_pattern, $.rest_pattern)),
        '}',
      ),

    field_pattern: ($) => seq(field('name', $.identifier), ':', field('pattern', $._pattern)),

    negative_literal: ($) => seq('-', choice($.integer, $.float)),

    // `error: FileError`. A colon in a pattern always introduces a type.
    typed_pattern: ($) =>
      prec(-1, seq(field('name', $.identifier), ':', field('type', $._type))),

    tuple_pattern: ($) =>
      seq('(', $._pattern, repeat1(seq(',', $._pattern)), optional(','), ')'),

    list_pattern: ($) => seq('[', commaSep(choice($._pattern, $.rest_pattern)), ']'),

    // `..rest` binds the remaining elements; a bare `..` ignores them.
    rest_pattern: ($) => prec.right(seq('..', optional(field('name', $.identifier)))),

    // `as` binds less tightly than every other pattern form.
    alias_pattern: ($) =>
      prec.left(seq(field('pattern', $._pattern), 'as', field('alias', $.identifier))),

    loop_expression: ($) =>
      seq(
        'loop',
        choice(plainState($), field('pattern', $._pattern)),
        '=',
        field('initial', $._expression),
        field('body', $.block),
      ),

    // A bare `break` may end a case arm, so a name on the next line can start
    // the following arm; both readings are kept until one fails.
    break_expression: ($) =>
      choice(
        prec('break_value', 'break'),
        prec.right(seq('break', field('value', $._expression))),
      ),

    function_expression: ($) =>
      seq(
        'fn',
        field('parameters', $.parameters),
        optional(field('return_type', $.return_type)),
        optional($.requirement_clause),
        field('body', $.block),
      ),

    // Tokens

    boolean: (_) => choice('True', 'False'),

    // `Void` is both the type and its only value.
    void_value: (_) => 'Void',

    integer: (_) => token(choice(/0[xX][0-9a-fA-F_]+/, /0[bB][01_]+/, /[0-9][0-9_]*/)),

    // A `.` starts a fraction only when a digit follows, so `0..10` stays a range.
    float: (_) =>
      token(
        choice(
          /[0-9][0-9_]*\.[0-9][0-9_]*([eE][+-]?[0-9][0-9_]*)?/,
          /[0-9][0-9_]*[eE][+-]?[0-9][0-9_]*/,
        ),
      ),

    text: ($) =>
      seq(
        '"',
        repeat(choice($.text_content, $.escape_sequence, $.interpolation)),
        token.immediate('"'),
      ),

    // The run takes precedence so `//` inside text is not a comment. A `$` not
    // followed by `{` is literal text; it has no precedence so the longer `${`
    // wins.
    text_content: (_) =>
      choice(token.immediate(prec(1, /[^"\\$\r\n]+/)), token.immediate('$')),

    escape_sequence: (_) => token.immediate(/\\(u\{[0-9a-fA-F]*\}|.)/),

    // `${expression}`; the expression may hold braces and nested text.
    interpolation: ($) => seq(token.immediate('${'), $._expression, '}'),

    identifier: (_) => /[A-Za-z_][A-Za-z0-9_]*/,

    comment: (_) => token(seq('//', /[^\r\n]*/)),
  },
});

// `name` or `name: Type`. It wins over the same text read as a pattern, so a
// simple parameter or binding keeps its `name` and `type` fields.
function plainBinding($) {
  return prec(
    1,
    seq(field('name', $.identifier), optional(seq(':', field('type', $._type)))),
  );
}

function plainState($) {
  return prec(1, field('state', $.identifier));
}

function sep1(rule, separator) {
  return seq(rule, repeat(seq(separator, rule)));
}
