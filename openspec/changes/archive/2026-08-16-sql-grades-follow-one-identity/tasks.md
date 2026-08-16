## 1. Correct the spec

- [x] 1.1 Rewrite `sql-graph`'s grading requirement to state what the shipped
  code does: one identity per reference, every edge `Exact`, an unqualified
  reference adopting the single qualifying schema and standing for itself when
  several qualify. → verify: no scenario in `sql-graph` asserts an `Ambiguous`
  table edge, and none asserts an edge per candidate.
- [x] 1.2 State the order-independence requirement, since it is what makes "how
  many schemas qualify this name" answerable at all. → verify: a scenario asserts
  the same identities whatever order files are walked.

## 2. Confirm no code change is owed

- [x] 2.1 The behaviour ships and is tested — `an_unqualified_reference_adopts_the_one_schema_that_qualifies_it`
  and `an_unqualified_reference_refuses_to_choose_between_two_schemas` in
  `sql::registry`, plus `a_name_with_two_schemas_resolves_the_same_in_any_order`
  in `xml_sql`. → verify: `just test` green with no source change in this change.
- [x] 2.2 Confirm `LinkGrade::Ambiguous` is still produced elsewhere, so the
  variant is not now dead. → verify: the graph reports its usages —
  `markdown::resolve_link`, `markdown::code_resolve::single_grade`,
  `html::classes::class_usage_edges`, `css::usage::resolve_usages`,
  `css::ingest::core::emit_extends_edges`, and `store::db::codes::link_grade_code`.
  Six sites, none of them a table path.
