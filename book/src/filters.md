# Filters

Values such as those obtained from variables can be post-processed
using **filters**.
Filters are applied to values using the pipe symbol (`|`) and may
have optional extra arguments in parentheses.
Filters can be chained, in which case the output from one filter
is passed to the next.

```
{{ "HELLO"|lower }}
```

**Note that the pipe symbol must not be surrounded by spaces**;
otherwise, it will be interpreted as the `BitOr` operator.

Reva has a collection of built-in filters, documented below, but can also include custom filters.
Additionally, the `json` filter is included in the built-in filters, but is disabled by default.
Enable it with Cargo features (see below for more information).

**Table of contents**

- **[Built-in filters][#built-in-filters]:**

    - [`abs`][#abs]
    - [`as_ref`][#as_ref]
    - [`capitalize`][#capitalize]
    - [`center`][#center]
    - [`deref`][#deref]
    - [`escape|e`][#escape]
    - [`filesizeformat`][#filesizeformat]
    - [`fmt`][#fmt]
    - [`format`][#format]
    - [`indent`][#indent]
    - [`join`][#join]
    - [`linebreaks`][#linebreaks]
    - [`linebreaksbr`][#linebreaksbr]
    - [`lower|lowercase`][#lower]
    - [`safe`][#safe]
    - [`title`][#title]
    - [`trim`][#trim]
    - [`truncate`][#truncate]
    - [`upper|uppercase`][#upper]
    - [`urlencode`][#urlencode]
    - [`wordcount`][#wordcount]

- **[Optional / feature gated filters][#optional-filters]:**
  [`json|tojson`][#json],

- **[Custom filters][#custom-filters]**

## Built-In Filters

[#built-in-filters]: #built-in-filters

### abs

[#abs]: #abs

Returns the absolute value.

```
{{ -2|abs }}
```

Output:

```
2
```

### as_ref

[#as_ref]: #as_ref

Creates a reference to the given argument.

```
{{ "a"|as_ref }}
{{ self.x|as_ref }}
```

will become:

```
&"a"
&self.x
```

### capitalize

[#capitalize]: #capitalize

Capitalize a value. The first character will be uppercase, all others lowercase:

```
{{ "hello"|capitalize }}
```

Output:

```
Hello
```

### center

[#center]: #center

Centers the value in a field of a given width:

```
-{{ "a"|center(5) }}-
```

Output:

```
-  a  -
```

### deref

[#deref]: #deref

Dereferences the given argument.

```
{% let s = String::from("a")|as_ref %}
{% if s|deref == String::from("b") %}
{% endif %}
```

will become:

```
let s = &String::from("a");
if *s == String::from("b") {}
```

### escape | e

[#escape]: #escape--e

Escapes HTML characters in strings:

```
{{ "Escape <>&"|e }}
```

Output:

```
Escape &lt;&gt;&amp;
```

Optionally, it is possible to specify and override which escaper is used.
Consider a template where the escaper is configured as [`escape = "none"`].
However, somewhere escaping using the HTML escaper is desired.
Then it is possible to override and use the HTML escaper like this:

```jinja
{{ "Don't Escape <>&"|escape }}
{{ "Don't Escape <>&"|e }}

{{ "Escape <>&"|escape("html") }}
{{ "Escape <>&"|e("html") }}
```

Output:

```text
Don't Escape <>&
Don't Escape <>&

Escape &lt;&gt;&amp;
Escape &lt;&gt;&amp;
```

[`escape = "none"`]: creating_templates.html#the-template-attribute

### filesizeformat

[#filesizeformat]: #filesizeformat

Returns adequate string representation (in KB, ..) of number of bytes:

```
{{ 1000|filesizeformat }}
```

Output:

```
1 KB
```

### fmt

[#fmt]: #fmt

Formats arguments according to the specified format

The _second_ argument to this filter must be a string literal (as in normal
Rust). The two arguments are passed through to [`format!()`] by
the Reva code generator, but the order is swapped to support filter
composition.

```text
{{ value|fmt("{:?}") }}
```

As an example, this allows filters to be composed like the following.
Which is not possible using the `format` filter.

```text
{{ value|capitalize|fmt("{:?}") }}
```

### format

[#format]: #format

Formats arguments according to the specified format.

The first argument to this filter must be a string literal (as in normal Rust).

All arguments are passed through to [`format!()`] by the Reva code generator.

```
{{ "{:?}"|format(var) }}
```

[`format!()`]: https://doc.rust-lang.org/stable/std/macro.format.html

### indent

[#indent]: #indent

Indent newlines with width spaces.

```
{{ "hello\nfoo\nbar"|indent(4) }}
```

Output:

```
hello
    foo
    bar
```

### join

[#join]: #join

Joins iterable into a string separated by provided argument.

```
array = &["foo", "bar", "bazz"]
```

```
{{ array|join(", ") }}
```

Output:

```
foo, bar, bazz
```

### linebreaks

[#linebreaks]: #linebreaks

Replaces line breaks in plain text with appropriate HTML.

A single newline becomes an HTML line break `<br>` and a new line followed by a blank line becomes a paragraph break `<p>`.

```
{{ "hello\nworld\n\nfrom\nreva"|linebreaks }}
```

Output:

```
<p>hello<br />world</p><p>from<br />reva</p>
```

### linebreaksbr

[#linebreaksbr]: #linebreaksbr

Converts all newlines in a piece of plain text to HTML line breaks.

```
{{ "hello\nworld\n\nfrom\nreva"|linebreaks }}
```

Output:

```
hello<br />world<br /><br />from<br />reva
```

### paragraphbreaks

[#paragraphbreaks]: #paragraphbreaks

A new line followed by a blank line becomes `<p>`, but, unlike `linebreaks`, single new lines are ignored and no `<br/>` tags are generated.

Consecutive double line breaks will be reduced down to a single paragraph break.

This is useful in contexts where changing single line breaks to line break tags would interfere with other HTML elements, such as lists and nested `<div>` tags.

```
{{ "hello\nworld\n\nfrom\n\n\n\nreva"|paragraphbreaks }}
```

Output:

```
<p>hello\nworld</p><p>from</p><p>reva</p>
```

### lower | lowercase

[#lower]: #lower--lowercase

Converts to lowercase.

```
{{ "HELLO"|lower }}
```

Output:

```
hello
```

### safe

[#safe]: #safe

Marks a string (or other Display type) as safe. By default all strings are escaped according to the format.

```
{{ "<p>I'm Safe</p>"|safe }}
```

Output:

```
<p>I'm Safe</p>
```

### title

[#title]: #title

Return a title cased version of the value. Words will start with uppercase letters, all
remaining characters are lowercase.

```
{{ "hello WORLD"|title }}
```

Output:

```
Hello World
```

### trim

[#trim]: #trim

Strip leading and trailing whitespace.

```
{{ " hello "|trim }}
```

Output:

```
hello
```

### truncate

[#truncate]: #truncate

Limit string length, appends '...' if truncated.

```
{{ "hello"|truncate(2) }}
```

Output:

```
he...
```

### upper | uppercase

[#upper]: #upper--uppercase

Converts to uppercase.

```
{{ "hello"|upper }}
```

Output:

```
HELLO
```

### urlencode

[#urlencode]: #urlencode

Percent encodes the string. Replaces reserved characters with the % escape character followed by a byte value as two hexadecimal digits.

```
hello?world
```

Output:

```
hello%3Fworld
```

### wordcount

[#wordcount]: #wordcount

Count the words in that string.

```
{{ "reva is sort of cool"|wordcount }}
```

Output:

```
5
```

## Optional / feature gated filters

[#optional-filters]: #optional--feature-gated-filters

The following filters can be enabled by requesting the respective feature in the Cargo.toml
[dependencies section](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html), e.g.

```
[dependencies]
reva = { version = "0.11.2", features = "serde-json" }
```

### `json` | `tojson`

[#json]: #json--tojson

Enabling the `serde-json` feature will enable the use of the `json` filter.
This will output formatted JSON for any value that implements the required
[`Serialize`](https://docs.rs/serde/1.*/serde/trait.Serialize.html) trait.
The generated string does not contain ampersands `&`, chevrons `< >`, or apostrophes `'`.

To use it in a `<script>` you can combine it with the safe filter.
In HTML attributes, you can either use it in quotation marks `"{{data|json}}"` as is,
or in apostrophes with the (optional) safe filter `'{{data|json|safe}}'`.
In HTML texts the output of e.g. `<pre>{{data|json|safe}}</pre>` is safe, too.

```
Good: <li data-extra="{{data|json}}">…</li>
Good: <li data-extra='{{data|json|safe}}'>…</li>
Good: <pre>{{data|json|safe}}</pre>
Good: <script>var data = {{data|json|safe}};</script>

Bad:  <li data-extra="{{data|json|safe}}">…</li>
Bad:  <script>var data = {{data|json}};</script>
Bad:  <script>var data = "{{data|json|safe}}";</script>

Ugly: <script>var data = "{{data|json}}";</script>
Ugly: <script>var data = '{{data|json|safe}}';</script>
```

## Custom Filters

[#custom-filters]: #custom-filters

To define your own filters, simply have a module named `filters` in scope of the context deriving a `Template` impl
and define the filters as functions within this module.
The functions must have at least one argument and the return type must be `::reva::Result<T>`.
Although there are no restrictions on `T` for a single filter,
the final result of a chain of filters must implement `Display`.

The arguments to the filters are passed as follows.
The first argument corresponds to the expression they are applied to.
Subsequent arguments, if any, must be given directly when calling the filter.
The first argument may or may not be a reference, depending on the context in which the filter is called.
To abstract over ownership, consider defining your argument as a trait bound.
For example, the `trim` built-in filter accepts any value implementing `Display`.
Its signature is similar to `fn trim(s: impl std::fmt::Display) -> ::reva::Result<String>`.

Note that built-in filters have preference over custom filters, so, in case of name collision, the built-in filter is applied.

### Examples

Implementing a filter that replaces all instances of `"oo"` for `"aa"`.

```rust
use reva::Template;

#[derive(Template)]
#[template(source = "{{ s|myfilter }}", ext = "txt")]
struct MyFilterTemplate<'a> {
    s: &'a str,
}

// Any filter defined in the module `filters` is accessible in your template.
mod filters {
    // This filter does not have extra arguments
    pub fn myfilter<T: std::fmt::Display>(s: T) -> ::reva::Result<String> {
        let s = s.to_string();
        Ok(s.replace("oo", "aa"))
    }
}

fn main() {
    let t = MyFilterTemplate { s: "foo" };
    assert_eq!(t.render().unwrap(), "faa");
}
```

Implementing a filter that replaces all instances of `"oo"` for `n` times `"a"`.

```rust
use reva::Template;

#[derive(Template)]
#[template(source = "{{ s|myfilter(4) }}", ext = "txt")]
struct MyFilterTemplate<'a> {
    s: &'a str,
}

// Any filter defined in the module `filters` is accessible in your template.
mod filters {
    // This filter requires a `usize` input when called in templates
    pub fn myfilter<T: std::fmt::Display>(s: T, n: usize) -> ::reva::Result<String> {
        let s = s.to_string();
    	  let mut replace = String::with_capacity(n);
    	  replace.extend((0..n).map(|_| "a"));
        Ok(s.replace("oo", &replace))
    }
}

fn main() {
    let t = MyFilterTemplate { s: "foo" };
    assert_eq!(t.render().unwrap(), "faaaa");
}
```
