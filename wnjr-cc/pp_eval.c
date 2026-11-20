// wnjr-cc: self-contained constant-expression evaluator for the C
// preprocessor's #if / #elif directives.
//
// In chibicc, preprocess.c calls const_expr() (defined in parse.c) to
// evaluate #if expressions. wnjr-cc does not carry parse.c, so this file
// provides const_expr() directly over the token stream. By the time
// preprocess.c calls us, "defined(...)" has been folded to 0/1, every
// remaining identifier has been replaced by 0, and pp-numbers have been
// converted to regular TK_NUM tokens. So the input is a C integer
// constant expression made of numbers and punctuators, which we evaluate
// with the standard operator grammar in signed 64-bit arithmetic (the
// #if evaluation type is intmax_t).
//
// Based on chibicc by Rui Ueyama (MIT license, see LICENSE).

#include "chibicc.h"

static int64_t cond_expr(Token **rest, Token *tok);
static int64_t assign_expr(Token **rest, Token *tok);

static int64_t primary(Token **rest, Token *tok) {
  if (equal(tok, "(")) {
    int64_t val = assign_expr(&tok, tok->next);
    *rest = skip(tok, ")");
    return val;
  }
  if (tok->kind == TK_NUM) {
    *rest = tok->next;
    return tok->val;
  }
  error_tok(tok, "expected an expression");
}

static int64_t unary(Token **rest, Token *tok) {
  if (equal(tok, "+"))
    return unary(rest, tok->next);
  if (equal(tok, "-"))
    return -unary(rest, tok->next);
  if (equal(tok, "!"))
    return !unary(rest, tok->next);
  if (equal(tok, "~"))
    return ~unary(rest, tok->next);
  return primary(rest, tok);
}

static int64_t mul(Token **rest, Token *tok) {
  int64_t val = unary(&tok, tok);
  for (;;) {
    if (equal(tok, "*")) { val = val * unary(&tok, tok->next); continue; }
    if (equal(tok, "/")) { val = val / unary(&tok, tok->next); continue; }
    if (equal(tok, "%")) { val = val % unary(&tok, tok->next); continue; }
    *rest = tok;
    return val;
  }
}

static int64_t add(Token **rest, Token *tok) {
  int64_t val = mul(&tok, tok);
  for (;;) {
    if (equal(tok, "+")) { val = val + mul(&tok, tok->next); continue; }
    if (equal(tok, "-")) { val = val - mul(&tok, tok->next); continue; }
    *rest = tok;
    return val;
  }
}

static int64_t shift(Token **rest, Token *tok) {
  int64_t val = add(&tok, tok);
  for (;;) {
    if (equal(tok, "<<")) { val = val << add(&tok, tok->next); continue; }
    if (equal(tok, ">>")) { val = val >> add(&tok, tok->next); continue; }
    *rest = tok;
    return val;
  }
}

static int64_t relational(Token **rest, Token *tok) {
  int64_t val = shift(&tok, tok);
  for (;;) {
    if (equal(tok, "<"))  { val = val <  shift(&tok, tok->next); continue; }
    if (equal(tok, "<=")) { val = val <= shift(&tok, tok->next); continue; }
    if (equal(tok, ">"))  { val = val >  shift(&tok, tok->next); continue; }
    if (equal(tok, ">=")) { val = val >= shift(&tok, tok->next); continue; }
    *rest = tok;
    return val;
  }
}

static int64_t equality(Token **rest, Token *tok) {
  int64_t val = relational(&tok, tok);
  for (;;) {
    if (equal(tok, "==")) { val = val == relational(&tok, tok->next); continue; }
    if (equal(tok, "!=")) { val = val != relational(&tok, tok->next); continue; }
    *rest = tok;
    return val;
  }
}

static int64_t bit_and(Token **rest, Token *tok) {
  int64_t val = equality(&tok, tok);
  while (equal(tok, "&"))
    val = val & equality(&tok, tok->next);
  *rest = tok;
  return val;
}

static int64_t bit_xor(Token **rest, Token *tok) {
  int64_t val = bit_and(&tok, tok);
  while (equal(tok, "^"))
    val = val ^ bit_and(&tok, tok->next);
  *rest = tok;
  return val;
}

static int64_t bit_or(Token **rest, Token *tok) {
  int64_t val = bit_xor(&tok, tok);
  while (equal(tok, "|"))
    val = val | bit_xor(&tok, tok->next);
  *rest = tok;
  return val;
}

static int64_t log_and(Token **rest, Token *tok) {
  int64_t val = bit_or(&tok, tok);
  while (equal(tok, "&&"))
    val = bit_or(&tok, tok->next) && val;
  *rest = tok;
  return val;
}

static int64_t log_or(Token **rest, Token *tok) {
  int64_t val = log_and(&tok, tok);
  while (equal(tok, "||"))
    val = log_and(&tok, tok->next) || val;
  *rest = tok;
  return val;
}

static int64_t cond_expr(Token **rest, Token *tok) {
  int64_t cond = log_or(&tok, tok);
  if (!equal(tok, "?")) {
    *rest = tok;
    return cond;
  }
  int64_t then = assign_expr(&tok, tok->next);
  tok = skip(tok, ":");
  int64_t els = cond_expr(&tok, tok);
  *rest = tok;
  return cond ? then : els;
}

static int64_t assign_expr(Token **rest, Token *tok) {
  // Preprocessor constant expressions have no assignment; the top of the
  // grammar is the conditional expression, with comma handled below.
  int64_t val = cond_expr(&tok, tok);
  while (equal(tok, ",")) {
    val = cond_expr(&tok, tok->next);
  }
  *rest = tok;
  return val;
}

int64_t const_expr(Token **rest, Token *tok) {
  return assign_expr(rest, tok);
}
