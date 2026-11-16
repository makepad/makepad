// with the `needed` feature the program calls this member, which needs helper.c's
extern int acu_helper(void);
int acu_needed_api(void) { return acu_helper() + 1; }
