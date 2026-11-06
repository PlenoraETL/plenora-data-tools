"""Controlli degli esiti Python sull'AST, con alias di import per scope."""

import ast


def python_findings(source: str) -> list[tuple[int, str, str]]:
    tree = ast.parse(source)
    found = []
    lines = source.splitlines()

    def add(node, rule):
        found.append((node.lineno, rule, lines[node.lineno - 1].strip()))

    def local_nodes(node):
        yield node
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda)):
            for child in ast.iter_child_nodes(node):
                yield from local_nodes(child)

    def scope(body, inherited):
        aliases = dict(inherited)
        nodes = [n for stmt in body for n in local_nodes(stmt)]
        for node in nodes:
            if isinstance(node, ast.Import):
                for item in node.names:
                    aliases[item.asname or item.name.split('.')[0]] = item.name
            elif isinstance(node, ast.ImportFrom):
                for item in node.names:
                    if item.name == '*' and node.module == 'pytest':
                        aliases['raises'] = 'pytest.raises'
                    else:
                        aliases[item.asname or item.name] = f'{node.module}.{item.name}'

        # Resolve import aliases at the point of use, not using later rebindings.
        import_nodes = [n for n in nodes if isinstance(n, (ast.Import, ast.ImportFrom))]
        final_aliases = aliases

        def aliases_at(position):
            result = dict(inherited)
            for item in import_nodes:
                if (item.lineno, item.col_offset) > position:
                    continue
                for entry in item.names:
                    if isinstance(item, ast.Import):
                        result[entry.asname or entry.name.split('.')[0]] = entry.name if entry.asname else entry.name.split('.')[0]
                    elif entry.name == '*' and item.module == 'pytest':
                        result['raises'] = 'pytest.raises'
                    else:
                        result[entry.asname or entry.name] = f'{item.module}.{entry.name}'
            return result

        def name(node):
            if isinstance(node, ast.Name):
                return aliases_at((node.lineno, node.col_offset)).get(node.id, node.id)
            if isinstance(node, ast.Attribute):
                return f'{name(node.value)}.{node.attr}'
            return ''

        def generic(node):
            if node is None:
                return True
            if isinstance(node, ast.Tuple):
                return any(generic(n) for n in node.elts)
            return name(node).split('.')[-1] in {'Exception', 'BaseException', 'PlenoraError'}

        def fixed_class(node):
            resolved = name(node).split('.')[-1]
            return bool(resolved) and not generic(node) and (
                resolved.endswith('Error') or resolved in {'KeyboardInterrupt', 'SystemExit'}
            )

        def category_assertion(node, value):
            if not isinstance(node, ast.Assert):
                return False
            test = node.test
            if isinstance(test, ast.Compare) and len(test.ops) == 1:
                left, right = test.left, test.comparators[0]
                for attr, expected in ((left, right), (right, left)):
                    if (isinstance(test.ops[0], ast.Eq) and isinstance(attr, ast.Attribute)
                            and attr.attr == 'category' and ast.dump(attr.value) == value
                            and isinstance(expected, ast.Constant) and isinstance(expected.value, str)):
                        return True
                if isinstance(test.ops[0], (ast.Is, ast.Eq)):
                    for call, expected in ((left, right), (right, left)):
                        if (isinstance(call, ast.Call) and name(call.func) in {'type', 'builtins.type'}
                                and len(call.args) == 1 and ast.dump(call.args[0]) == value
                                and fixed_class(expected)):
                            return True
            return (isinstance(test, ast.Call) and name(test.func) in {'isinstance', 'builtins.isinstance'}
                    and len(test.args) == 2 and ast.dump(test.args[0]) == value
                    and fixed_class(test.args[1]))

        def proven(value):
            if any(category_assertion(n, value) for n in nodes):
                return True
            for node in nodes:
                if isinstance(node, (ast.With, ast.AsyncWith)):
                    for item in node.items:
                        call = item.context_expr
                        if (isinstance(call, ast.Call) and name(call.func) == 'pytest.raises'
                                and call.args and fixed_class(call.args[0]) and item.optional_vars):
                            binding = ast.parse(ast.unparse(item.optional_vars), mode='eval').body
                            attr = ast.Attribute(value=binding, attr='value', ctx=ast.Load())
                            if ast.dump(attr) == value:
                                return True
            return False

        for node in nodes:
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                scope(node.body, final_aliases)
                if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    extras = node.decorator_list + node.args.defaults + [v for v in node.args.kw_defaults if v is not None]
                    scope([ast.Expr(value=v) for v in extras], aliases_at((node.lineno, node.col_offset)))
                else:
                    extras = node.decorator_list + node.bases + [k.value for k in node.keywords]
                    scope([ast.Expr(value=v) for v in extras], aliases_at((node.lineno, node.col_offset)))
            if isinstance(node, ast.Lambda):
                scope([ast.Expr(value=v) for v in [node.body, *node.args.defaults, *[d for d in node.args.kw_defaults if d is not None]]], aliases_at((node.lineno, node.col_offset)))
            if isinstance(node, ast.Call):
                target = name(node.func)
                if target in {'zip', 'builtins.zip'} and not any(
                    k.arg == 'strict' and isinstance(k.value, ast.Constant) and k.value.value is True
                    for k in node.keywords
                ):
                    add(node, 'zip_python_troncante')
                if target == 'pytest.raises' and (not node.args or generic(node.args[0])
                                                 or isinstance(node.args[0], ast.Tuple)):
                    add(node, 'raises_generico')
            if isinstance(node, ast.ExceptHandler) and generic(node.type):
                handler_nodes = node.body
                value = ast.dump(ast.Name(id=node.name, ctx=ast.Load())) if node.name else None
                if not any(isinstance(n, ast.Raise) or (value and category_assertion(n, value))
                           for n in handler_nodes):
                    add(node, 'except_ignorato')
            if isinstance(node, ast.Assert):
                values = [ast.dump(n.args[0]) for n in ast.walk(node.test)
                          if isinstance(n, ast.Call) and name(n.func) in {'str', 'builtins.str'} and n.args]
                if values and not all(proven(value) for value in values):
                    add(node, 'messaggio_python')
    scope(tree.body, {})
    return sorted(found)
