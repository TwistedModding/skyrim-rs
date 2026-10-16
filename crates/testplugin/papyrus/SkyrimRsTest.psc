Scriptname SkyrimRsTest Hidden
{Declarations of the natives crates/testplugin registers through skyrim::papyrus; compiled by scripts/compile-papyrus.ps1.}

int Function Ping(int x) native global
int Function FormId(Form akForm) native global
float Function PlayerHealth(Actor akActor) native global
string Function Echo(string s) native global
float Function Scale(float x, bool double) native global
Form Function Player() native global
Function Tick() native global
string Function Describe(ObjectReference akRef, int count, string suffix, bool flag) native global
float[] Function Halves(int[] values) native global
Form[] Function Forms() native global
int Function DoubleNow(int x) native global
int Function DoubleLater(int x) native global
Function RecordEvent(int count, string text) native global
int Function EventCount() native global
