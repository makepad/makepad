// taken only through needed.c: its missing symbol is then an undefined symbol of the program
extern int acu_missing_needed(void);
int acu_helper(void) { return acu_missing_needed(); }
