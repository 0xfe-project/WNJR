// Designated initializers that name a member through more than one level, and
// that continue with another member of an enclosing aggregate afterwards. Every
// value below is checked against the host compiler, so a field filled in the
// wrong place cannot pass.

struct clock {
  int id;
  int flags;
};

struct subscription {
  int tag;
  struct clock u;
};

struct record {
  int userdata;
  struct subscription sub;
  int tail;
};

struct record r = {
    .sub.tag = 7,
    .sub.u.id = 11,
    .sub.u.flags = 13,
    .userdata = 5,
    .tail = 17,
};

int record_designators(void) {
  return r.userdata * 100000000 + r.sub.tag * 1000000 + r.sub.u.id * 10000 +
         r.sub.u.flags * 100 + r.tail;
}

// The same shape inside a union member, which reaches its member through the
// union's own designator.
union holder {
  struct clock c;
  long long raw;
};

// A union initializer names one member, so there is no second designator here;
// what this checks is that the path reaches through the union to the struct.
union holder h = {.c.flags = 4};

int union_designators(void) { return h.c.id * 100 + h.c.flags; }

// An array whose elements are designated one by one, with a member path inside
// each designator.
struct record arr[3] = {
    [0].sub.u.id = 21,
    [2].userdata = 22,
    [2].sub.u.flags = 23,
};

int array_designators(void) {
  return arr[0].sub.u.id * 10000 + arr[1].userdata * 100 + arr[2].userdata +
         arr[2].sub.u.flags;
}

// A positional element followed by a designator for an enclosing member: the
// comma before ".tail" separates elements of the outer initializer.
struct record mix = {1, {2, {3, 4}}, .tail = 6};

int mixed_designators(void) {
  return mix.userdata * 1000000 + mix.sub.tag * 10000 + mix.sub.u.id * 100 +
         mix.sub.u.flags * 10 + mix.tail;
}
