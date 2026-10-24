; The HCL crate supplies a parser but no highlighting query.
(comment) @comment
(string_lit) @string
(quoted_template) @string
(heredoc_template) @string
(numeric_lit) @number
(bool_lit) @boolean
(null_lit) @constant.builtin
(block . (identifier) @type)
(attribute (identifier) @attribute)
(function_call (identifier) @function)
(get_attr (identifier) @attribute)
["for" "in" "if" "else" "endif" "endfor"] @keyword
["=" "+" "-" "*" "/" "%" "!" "==" "!=" ">" ">=" "<" "<=" "&&" "||" "?" ":" "=>"] @operator
