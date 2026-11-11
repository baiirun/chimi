/**
 * Tree-sitter grammar for Chimi, used for editor highlighting and structure.
 *
 * The compiler's lossless parser in `compiler/chimi/src/syntax.rs` is the
 * authority. This grammar is deliberately more permissive: line breaks are
 * whitespace here, so a call or `break` value on the following line parses
 * differently than in the compiler. That only affects highlighting of code the
 * compiler already reports.
 */

const PREC = {
  using: 1,
  pipeline: 2,
  comparison: 3,
  additive: 4,
  multiplicative: 5,
  unary: 6,
  postfix: 7,
};

const commaSep1 = (rule) => seq(rule, repeat(seq(',', rule)), optional(','));
const commaSep = (rule) => optional(commaSep1(rule));

module.exports = grammar({
  name: 'chimi',

  extras: ($) => [/\s/, $.comment],

  word: ($) => $.identifier,

  supertypes: ($) => [$._expression, $._type],

  rules: {
    source_file: ($) => repeat($._item),

    _item: ($) =>
      choice(
        $.import_declaration,
        $.struct_declaration,
        $.enum_declaration,
        $.interface_declaration,
        $.function_declaration,
        $.let_binding,
        $._expression,
      ),

    // Declarations

    import_declaration: ($) =>
      seq(
        'import',
        field('path', $.module_path),
        choice(
          seq('as', field('alias', $.identifier)),
          seq('.', '{', commaSep1(field('member', $.identifier)), '}'),
        ),
      ),

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

    generic_parameters: ($) =>
      seq('<', commaSep1(field('name', $.identifier)), '>'),

    function_declaration: ($) =>
      seq(
        optional($.visibility),
        'fn',
        field('name', $.identifier),
        optional($.generic_parameters),
        field('parameters', $.parameters),
        optional(field('return_type', $.return_type)),
        optional($.requirement_clause),
        field('body', $.block),
      ),

    parameters: ($) => seq('(', commaSep($.parameter), ')'),

    parameter: ($) =>
      seq(field('name', $.identifier), optional(seq(':', field('type', $._type)))),

    return_type: ($) => seq('->', $._type),

    requirement_clause: ($) =>
      prec.right(
        seq(
          'needs',
          $._requirement,
          repeat(seq('+', $._requirement)),
        ),
      ),

    _requirement: ($) => choice(alias($.identifier, $.type_identifier), $.requirement_record),

    requirement_record: ($) => seq('{', commaSep1($.field), '}'),

    // Types

    _type: ($) => choice($.named_type, $.function_type, $.void_type),

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

    function_type: ($) =>
      prec.right(
        seq(
          'fn',
          '(',
          commaSep($._type),
          ')',
          '->',
          field('return_type', $._type),
          optional(seq('errors', field('errors', $._type))),
          optional($.requirement_clause),
        ),
      ),

    // Bindings and expressions

    let_binding: ($) =>
      seq('let', field('name', $.identifier), '=', field('value', $._expression)),

    _expression: ($) =>
      choice(
        $.using_expression,
        $.pipeline_expression,
        $.binary_expression,
        $.unary_expression,
        $.call_expression,
        $.member_expression,
        $.integer,
        $.text,
        $.boolean,
        $.identifier,
        $.parenthesized_expression,
        $.list_expression,
        $.record_expression,
        $.block,
        $.case_expression,
        $.loop_expression,
        $.break_expression,
        $.function_expression,
      ),

    using_expression: ($) =>
      prec.left(
        PREC.using,
        seq(field('operation', $._expression), 'using', field('providers', $.record_expression)),
      ),

    pipeline_expression: ($) =>
      prec.left(
        PREC.pipeline,
        seq(field('value', $._expression), '|>', field('transform', $._expression)),
      ),

    binary_expression: ($) => {
      const table = [
        [PREC.comparison, choice('<', '<=', '>', '>=')],
        [PREC.additive, choice('+', '-')],
        [PREC.multiplicative, choice('*', '/')],
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

    unary_expression: ($) =>
      prec(PREC.unary, seq(field('operator', '-'), field('operand', $._expression))),

    call_expression: ($) =>
      prec(PREC.postfix, seq(field('function', $._expression), field('arguments', $.arguments))),

    arguments: ($) => seq('(', commaSep($.argument), ')'),

    argument: ($) =>
      seq(optional(seq(field('name', $.identifier), ':')), field('value', $._expression)),

    member_expression: ($) =>
      prec(
        PREC.postfix,
        seq(field('object', $._expression), '.', field('member', $.identifier)),
      ),

    parenthesized_expression: ($) => seq('(', $._expression, ')'),

    list_expression: ($) => seq('[', commaSep($._expression), ']'),

    record_expression: ($) => seq('{', commaSep1($.record_field), '}'),

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
      prec.right(
        seq(
          commaSep1(field('pattern', $._pattern)),
          optional(seq('if', field('guard', $._expression))),
          '->',
          field('body', $._expression),
        ),
      ),

    _pattern: ($) => choice($.integer, $.text, $.boolean, $.constructor_pattern),

    constructor_pattern: ($) =>
      prec.right(
        seq(
          sep1($.identifier, '.'),
          optional(seq('(', commaSep($._pattern), ')')),
        ),
      ),

    loop_expression: ($) =>
      seq(
        'loop',
        field('state', $.identifier),
        '=',
        field('initial', $._expression),
        field('body', $.block),
      ),

    break_expression: ($) => prec.right(seq('break', optional(field('value', $._expression)))),

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

    integer: (_) => /[0-9][0-9_]*/,

    text: ($) =>
      seq(
        '"',
        repeat(choice(token.immediate(prec(1, /[^"\\\r\n]+/)), $.escape_sequence)),
        token.immediate('"'),
      ),

    escape_sequence: (_) => token.immediate(/\\./),

    identifier: (_) => /[A-Za-z_][A-Za-z0-9_]*/,

    comment: (_) => token(seq('//', /[^\r\n]*/)),
  },
});

function sep1(rule, separator) {
  return seq(rule, repeat(seq(separator, rule)));
}
