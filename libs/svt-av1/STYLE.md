# Code Style

This style guide is derived from [`dav1d`](https://code.videolan.org/videolan/dav1d/wikis/Coding-style) with modifications. The authoritative source is the `.clang-format` file in this directory.

## Tabs vs Spaces

**No tabs,** only spaces; 4-space indentation.

Be aware that some tools might add tabs when auto aligning the code, please check your commits with a diff tool for tabs.

## Line Length

Lines should not exceed **120 characters**. Exceptions may be allowed on a case-by-case basis if wrapping would lead to exceptional ugliness.

## Indentation and Alignment

- Continuation indent is 4 spaces
- Align code after open brackets:

``` c
int result = my_function(argument1,
                         argument2,
                         argument3);
```

- Align consecutive assignments and declarations:

``` c
int    short_var  = 1;
int    longer_var = 2;
double other_var  = 3.0;
```

- Binary operators stay at the end of the line when breaking:

``` c
const int my_var = something1 +
    something2 - something3 * something4;
```

- Break before ternary operators:

``` c
int result = condition
    ? value_if_true
    : value_if_false;
```

## Naming Conventions

Use `CamelCase` for types and `under_score` for variable names (`TypeName my_instance;`).

## Pointer Alignment

Pointers are aligned to the left (next to the type):

``` c
int* pointer;
const char* string;
```

## Const Usage

Use const where possible, except in forward function declarations in header files, where we only use it for const-arrays:

``` c
int my_func(const array* values, int arg);

[..]

int my_func(const array* const values, const int num) {
    [..]
}
```

## Braces

**Braces are always required** for control statements (if, else, for, while, do, switch). This is enforced by clang-format with `InsertBraces: true`.

``` c
// Correct
if (condition) {
    do_something();
}

// Wrong - braces required even for single statements
if (condition)
    do_something();
```

Opening braces go on the same line as the statement:

``` c
static void function(const int argument) {
    do_something();
}

if (condition) {
    do_something();
} else {
    do_something_else();
}
```

For multi-line function declarations, the brace still stays on the same line:

``` c
static void function(const int argument1,
                     const int argument2) {
    do_something();
}
```

## Switch/Case

Case labels are at the same indentation level as the switch statement. Single-line case labels are **not allowed**:

``` c
switch (a) {
case 1:
    bla();
    break;
case 2:
    foo();
    break;
default:
    bar();
    break;
}
```

## Functions

- Short functions (empty body only) may be on a single line
- All parameters of a declaration can be on the next line
- Function names are not indented when wrapped

## Spacing

- Space after control statement keywords (`if (`, `for (`, `while (`)
- No space after C-style casts: `(int)value`
- No spaces inside parentheses: `func(arg)` not `func( arg )`
- Space before assignment operators: `x = 1`
- One space before trailing comments

## Empty Lines

- Maximum of 1 consecutive empty line
- No empty lines at the start of blocks
- Separate definition blocks with empty lines

## Comments

- Comments are not automatically reflowed
- Trailing comments have 1 space before them

## Includes

- Includes are **not** automatically sorted (to preserve intentional ordering)
- Include categories are prioritized: system headers, then project headers

## Other Guidelines

Don't use `goto` except for standard error handling.\
Use native types (`int`, `unsigned`, etc.) for scalar variables where the upper bound of a size doesn't matter.\
Use sized types (`uint8_t`, `int16_t`, etc.) for vector/array variables where the upper bound of the size matters.\
Use dynamic types (`pixel`, `coef`, etc.) so multi-bitdepth templating works as it should.

## Nomenclature conventions

With quite a bit of code being shared between libaom, libdav1d and SVT-AV1, build conflicts may arise these libraries are linked
 statically in the same build. So if your work involved porting code from other libraries (assuming a compatible license), please
 use the following nomenclature convention:


 - Add ```svt_av1``` before any public API (any function that can be accessed outside of the library).
 - Add ```svt_aom``` before any symbol that won't be publicly accessible.

## Doxygen Documentation

``` c
/* File level Description */

/*********************************************************************************
* @file
*  file.c
*
* @brief
*  Brief description about file
*
* @author
*  Author
*
* @par List of Functions:
* - fun1()
* - fun2()
*
* @remarks
*  Any remarks
*
********************************************************************************/

/* Macro Description */
/** Brief description of MACRO */
#define MACRO   val

/* enum Description : description for all entries */
/** Brief description of ENUMs */
enum {
    ENUM1         = 1, /**< Brief description of ENUM1 */
    ENUM2         = 2, /**< Brief description of ENUM2 */
    ENUM3         = 3  /**< Brief description of ENUM3 */
}

/* enum Description : top level description */
/** Brief description of ENUMs */
enum {
    ENUM1         = 1,
    ENUM2         = 2,
    ENUM3         = 3
}

/* structure level Description */

struct {
    member1, /**< Brief description of member1 */
    member2, /**< Brief description of member2 */
    member3, /**< Brief description of member3 */
}

/* Function level Description */

/*********************************************************************************
*
* @brief
*  Brief description of function
*
* @par Description:
*  Detailed description of function
*
* @param[in] prm1
*  Brief description of prm1
*
* @param[in] prm2
*  Brief description of prm2
*
* @param[out] prm3
*  Brief description of prm3
*
* @returns  Brief description of return value
*
* @remarks
*  Any remarks
*
********************************************************************************/
```

## Post-coding

After coding, make sure to trim any trailing white space and convert any tabs to 4 spaces

### For bash

``` bash
find . -name <Filename> -type f -exec sed -i 's/\t/    /;s/[[:space:]]*$//' {} +
```

Where `<Filename>` is `"*.c"` or `"*.(your file extention here)"`\
Search the `find` man page or tips and tricks for more options.\
**Do not** use find on root without a filter or with the `.git` folder still present. Doing so will corrupt your repo folder and you will need to copy a new `.git` folder and re-setup your folder.

Alternatively, for single file(s):

``` bash
sed -i 's/\t/    /;s/[[:space:]]*$//' <Filename/Filepath>
```

Note: For macOS and BSD related distros, you may need to use `sed -i ''` inplace due to differences with GNU sed.

### For Powershell/pwsh

``` Powershell
ls -Recurse -File -Filter *.c | ForEach-Object{$(Get-Content $_.FullName | Foreach {Write-Output "$($_.TrimEnd().Replace("`t","    "))`n"}) | Set-Content -NoNewline -Encoding utf8 $_.FullName}
```

Where `-Filter *.c` has your extention/filename(s).\
This does not work with `pwsh` on non-windows OS.\
Search the docs for [`pwsh`](https://docs.microsoft.com/en-us/powershell/scripting/overview?view=powershell-6) related commands and [`powershell`](https://docs.microsoft.com/en-us/powershell/scripting/overview?view=powershell-5.1) related commands for more information on what they do.\
**Do not** use ls without a `-Filter` on the root directory or with the `.git` folder still present. Doing so will corrupt your repo folder and you will need to copy a new `.git` folder and re-setup your folder.

Alternatively, for a single file:

``` Powershell
$filename="<filename>"; Get-content $filename | Foreach {Write-Output "$($_.TrimEnd().Replace("`t","    "))`n"}) | Set-Content -NoNewline $filename
```

Where `<filename>` is the specific file you want to trim.
