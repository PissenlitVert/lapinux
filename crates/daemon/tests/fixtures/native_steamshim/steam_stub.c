#include <stdbool.h>

/* Symbol shared with the bridge: without LD_PRELOAD this is the implementation
   selected by the dynamic loader. */
bool SteamAPI_Init(void) { return false; }
