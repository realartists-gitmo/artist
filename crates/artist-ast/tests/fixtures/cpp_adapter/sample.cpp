// Fixture for the C++ adapter integration tests.
namespace geom {

struct Point {
    int x;
    int y;
};

enum class Color { Red, Green, Blue };

class Shape {
public:
    Shape(int sides);
    ~Shape();
    int sides() const;
    virtual double area() const = 0;
private:
    int sides_;
};

}  // namespace geom

int free_function(int n) {
    return n + 1;
}
