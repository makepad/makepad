// a member nothing the program calls needs: a linker leaves it out, and its missing symbol with it
extern int acu_missing_optional(void);
int acu_unused_api(void) { return acu_missing_optional(); }
