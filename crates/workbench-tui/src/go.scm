; Specific captures must follow the bundled generic variable/property patterns.
(function_declaration name: (identifier) @function)
(method_declaration name: (field_identifier) @function.method)
(call_expression function: (identifier) @function)
(call_expression function: (selector_expression field: (field_identifier) @function.method))
(call_expression
  function: (identifier) @function.builtin
  (#match? @function.builtin "^(append|cap|clear|close|complex|copy|delete|imag|len|make|max|min|new|panic|print|println|real|recover)$"))
(parameter_declaration name: (identifier) @variable.parameter)
(variadic_parameter_declaration name: (identifier) @variable.parameter)
