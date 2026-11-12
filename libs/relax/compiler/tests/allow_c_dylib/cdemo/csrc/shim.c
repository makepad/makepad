/* the static shim: C code that calls the shared library */
int demo_answer(void);

int shim_answer(void) {
    return demo_answer();
}
