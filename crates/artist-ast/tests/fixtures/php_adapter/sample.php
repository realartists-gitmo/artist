<?php
// Fixture for the PHP adapter integration tests.
namespace App\Billing;

interface Payable {
    public function charge(int $cents): bool;
}

abstract class Account {
    public string $email;
    protected static int $count = 0;
    private const VERSION = 1;

    public function __construct(string $email) {
        $this->email = $email;
    }

    abstract public function balance(): int;

    private static function bump(): void {
        self::$count++;
    }
}

function format_amount(int $cents): string {
    return sprintf('$%0.2f', $cents / 100);
}
