// wnjr-cc: --dump-ast. Stable text dump of the AST produced by parse().
//
// This file is compiled unchanged into both wnjr-cc and a scratch copy of
// vendor chibicc (see test/vendor-dump.patch), so the two front ends can be
// compared byte for byte.
//
// Format: one line per object or node. Expression/statement trees are
// indented two spaces per level. A child line starts with its role in the
// parent ("lhs:", "cond:", "arg:", "stmt:" ...) followed by the node kind
// and "ty=<type>" then the key fields of that kind. No pointers are printed;
// labels are the parser's own deterministic ".L..N" names.

#include "chibicc.h"

static const char *node_names[] = {
  "NULL_EXPR", "ADD", "SUB", "MUL", "DIV", "NEG", "MOD", "BITAND", "BITOR",
  "BITXOR", "SHL", "SHR", "EQ", "NE", "LT", "LE", "ASSIGN", "COND", "COMMA",
  "MEMBER", "ADDR", "DEREF", "NOT", "BITNOT", "LOGAND", "LOGOR", "RETURN",
  "IF", "FOR", "DO", "SWITCH", "CASE", "BLOCK", "GOTO", "GOTO_EXPR", "LABEL",
  "LABEL_VAL", "FUNCALL", "EXPR_STMT", "STMT_EXPR", "VAR", "VLA_PTR", "NUM",
  "CAST", "MEMZERO", "ASM", "CAS", "EXCH", "TRAP", "CLZ", "CTZ", "POPCOUNT",
  "ISNAN", "MEMCPY", "MEMSET",
};

// This table is indexed by NodeKind and has to list every kind, in order. There
// is no assertion for that here on purpose: dump.c is also compiled against
// vendor chibicc's chibicc.h for test/dump-test.sh, and that header has fewer
// kinds, so the check cannot be written against ND_LAST. A kind added without a
// name prints whatever string follows the table.

static const char *type_names[] = {
  "void", "bool", "char", "short", "int", "long", "float", "double",
  "ldouble", "enum", "ptr", "func", "array", "vla", "struct", "union",
};

static FILE *out;

static void print_type(Type *ty, bool full);

static void print_member(Member *m) {
  if (m->name)
    fprintf(out, "%.*s", m->name->len, m->name->loc);
  else
    fprintf(out, "<anon>");
  fprintf(out, ":");
  print_type(m->ty, false);
  fprintf(out, "@%d", m->offset);
  if (m->is_bitfield)
    fprintf(out, ":bit%d+%d", m->bit_offset, m->bit_width);
}

// "full" expands struct/union members one level; nested aggregates are
// printed shallow so that self-referential structs terminate.
static void print_type(Type *ty, bool full) {
  if (!ty) {
    fprintf(out, "-");
    return;
  }
  if (ty->is_atomic)
    fprintf(out, "_Atomic ");
  if (ty->is_unsigned)
    fprintf(out, "u");

  switch (ty->kind) {
  case TY_PTR:
    fprintf(out, "ptr(");
    print_type(ty->base, false);
    fprintf(out, ")");
    return;
  case TY_ARRAY:
    fprintf(out, "array[%d](", ty->array_len);
    print_type(ty->base, false);
    fprintf(out, ")");
    return;
  case TY_VLA:
    fprintf(out, "vla(");
    print_type(ty->base, false);
    fprintf(out, ")");
    if (ty->vla_size)
      fprintf(out, "#%s", ty->vla_size->name);
    return;
  case TY_FUNC:
    fprintf(out, "func(");
    print_type(ty->return_ty, false);
    fprintf(out, ";");
    for (Type *p = ty->params; p; p = p->next) {
      print_type(p, false);
      if (p->next)
        fprintf(out, ",");
    }
    if (ty->is_variadic)
      fprintf(out, ",...");
    fprintf(out, ")");
    return;
  case TY_STRUCT:
  case TY_UNION:
    fprintf(out, "%s(size=%d,align=%d", type_names[ty->kind], ty->size, ty->align);
    if (ty->is_flexible)
      fprintf(out, ",flex");
    if (ty->is_packed)
      fprintf(out, ",packed");
    if (full) {
      fprintf(out, ";");
      for (Member *m = ty->members; m; m = m->next) {
        print_member(m);
        if (m->next)
          fprintf(out, ",");
      }
    }
    fprintf(out, ")");
    return;
  default:
    fprintf(out, "%s", type_names[ty->kind]);
    return;
  }
}

static void indent(int depth) {
  for (int i = 0; i < depth; i++)
    fprintf(out, "  ");
}

static void dump_node(Node *node, const char *role, int depth);

static void dump_list(Node *head, const char *role, int depth) {
  for (Node *n = head; n; n = n->next)
    dump_node(n, role, depth);
}

static void print_str(const char *s) {
  fputc('"', out);
  for (; *s; s++) {
    unsigned char c = *s;
    if (c == '"' || c == '\\')
      fprintf(out, "\\%c", c);
    else if (c < 32 || c >= 127)
      fprintf(out, "\\x%02x", c);
    else
      fputc(c, out);
  }
  fputc('"', out);
}

static void dump_node(Node *node, const char *role, int depth) {
  indent(depth);
  fprintf(out, "%s: %s ty=", role, node_names[node->kind]);
  print_type(node->ty, false);

  switch (node->kind) {
  case ND_NUM:
    if (node->ty && is_flonum(node->ty))
      fprintf(out, " fval=%.21Lg", node->fval);
    else
      fprintf(out, " val=%lld", (long long)node->val);
    break;
  case ND_VAR:
  case ND_VLA_PTR:
  case ND_MEMZERO:
    fprintf(out, " var=%s %s", node->var->name,
            node->var->is_local ? "local" : "global");
    break;
  case ND_MEMBER:
    fprintf(out, " member=");
    print_member(node->member);
    break;
  case ND_GOTO:
  case ND_LABEL:
  case ND_LABEL_VAL:
    if (node->label)
      fprintf(out, " label=%s", node->label);
    fprintf(out, " unique=%s", node->unique_label ? node->unique_label : "-");
    break;
  case ND_GOTO_EXPR:
    break;
  case ND_CASE:
    fprintf(out, " begin=%ld end=%ld unique=%s", node->begin, node->end,
            node->label ? node->label : "-");
    break;
  case ND_SWITCH:
    fprintf(out, " brk=%s default=%s cases=[", node->brk_label,
            node->default_case ? node->default_case->label : "-");
    for (Node *c = node->case_next; c; c = c->case_next)
      fprintf(out, "%s%s", c->label, c->case_next ? "," : "");
    fprintf(out, "]");
    break;
  case ND_FOR:
  case ND_DO:
    fprintf(out, " brk=%s cont=%s", node->brk_label ? node->brk_label : "-",
            node->cont_label ? node->cont_label : "-");
    break;
  case ND_FUNCALL:
    fprintf(out, " func_ty=");
    print_type(node->func_ty, false);
    if (node->ret_buffer)
      fprintf(out, " ret_buffer=%s", node->ret_buffer->name);
    break;
  case ND_ASM:
    fprintf(out, " asm=");
    print_str(node->asm_str);
    break;
  case ND_CAS:
    break;
  default:
    break;
  }
  if (node->atomic_addr)
    fprintf(out, " atomic_addr=%s", node->atomic_addr->name);
  fprintf(out, "\n");

  if (node->lhs)
    dump_node(node->lhs, "lhs", depth + 1);
  if (node->rhs)
    dump_node(node->rhs, "rhs", depth + 1);
  if (node->cond)
    dump_node(node->cond, "cond", depth + 1);
  if (node->then)
    dump_node(node->then, "then", depth + 1);
  if (node->els)
    dump_node(node->els, "else", depth + 1);
  if (node->init)
    dump_node(node->init, "init", depth + 1);
  if (node->inc)
    dump_node(node->inc, "inc", depth + 1);
  if (node->cas_addr)
    dump_node(node->cas_addr, "cas_addr", depth + 1);
  if (node->cas_old)
    dump_node(node->cas_old, "cas_old", depth + 1);
  if (node->cas_new)
    dump_node(node->cas_new, "cas_new", depth + 1);
  if (node->atomic_expr)
    dump_node(node->atomic_expr, "atomic_expr", depth + 1);
  // BLOCK and STMT_EXPR hold a statement chain; FUNCALL holds an argument chain.
  // For ND_CASE/ND_LABEL the sub-statement is in lhs, already handled above.
  dump_list(node->body, "stmt", depth + 1);
  dump_list(node->args, "arg", depth + 1);
}

static void dump_var_header(Obj *var, const char *what) {
  fprintf(out, "%s %s ty=", what, var->name);
  print_type(var->ty, true);
  fprintf(out, " align=%d", var->align);
}

static void dump_obj(Obj *obj) {
  if (obj->is_function) {
    dump_var_header(obj, "func");
    fprintf(out, "%s%s%s%s%s%s\n",
            obj->is_definition ? " def" : " decl",
            obj->is_static ? " static" : "",
            obj->is_inline ? " inline" : "",
            obj->is_live ? " live" : "",
            obj->is_root ? " root" : "",
            obj->va_area ? " variadic-area" : "");
    for (Obj *p = obj->params; p; p = p->next) {
      indent(1);
      dump_var_header(p, "param");
      fprintf(out, "\n");
    }
    // locals are prepended while parsing, so the list is newest-first.
    for (Obj *l = obj->locals; l; l = l->next) {
      // __va_buf__ holds the address of the caller's argument buffer and
      // __va_args__ is that buffer at a call site. Both are this frontend's own
      // machinery — chibicc passes variadic arguments in registers and va_arg
      // walks the save area — so printing them would make every variadic
      // function differ from chibicc's dump for a reason that is not about the C
      // being parsed.
      if (l->name &&
          (!strcmp(l->name, "__va_buf__") || !strcmp(l->name, "__va_args__")))
        continue;
      indent(1);
      dump_var_header(l, "local");
      fprintf(out, "\n");
    }
    if (obj->body)
      dump_node(obj->body, "body", 1);
    return;
  }

  dump_var_header(obj, "global");
  fprintf(out, "%s%s%s%s%s\n",
          obj->is_definition ? " def" : " decl",
          obj->is_static ? " static" : "",
          obj->is_tentative ? " tentative" : "",
          obj->is_tls ? " tls" : "",
          obj->is_live ? " live" : "");
  if (obj->init_data) {
    indent(1);
    fprintf(out, "data:");
    for (int i = 0; i < obj->ty->size; i++)
      fprintf(out, " %02x", (unsigned char)obj->init_data[i]);
    fprintf(out, "\n");
  }
  for (Relocation *r = obj->rel; r; r = r->next) {
    indent(1);
    fprintf(out, "reloc: off=%d label=%s addend=%ld\n", r->offset, *r->label,
            r->addend);
  }
}

void dump_ast(Obj *prog, FILE *f) {
  out = f;
  for (Obj *obj = prog; obj; obj = obj->next)
    dump_obj(obj);
}
