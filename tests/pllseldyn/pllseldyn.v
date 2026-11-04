// PLL_ADV with CLKINSEL driven from logic (ISE cross-check only). Both inputs
// come from clock pins through their own BUFIO2s; the PLL's input pair mux
// only takes two BUFIO2s together as DIVCLK_CMT_V[n] and V[n + 4], which on
// the TQG144's bottom edge means the two pins of one clock pair: P56 (the
// Mojo's 50 MHz clock) on CLKIN1 and P55 on CLKIN2. On the Mojo P55 is the
// AVR's serial line, so this design isn't for the board.
// rst_n selects the input (1: CLKIN1); the LEDs show a CLKOUT0 counter and
// LOCKED.
module pllseldyn (
    input  wire       clk,
    input  wire       clk2,
    input  wire       rst_n,
    output wire [7:0] led
);
    reg sel = 1;
    always @(posedge clk) sel <= rst_n;
    wire fb, o0, locked;
    PLL_ADV #(
        .CLKIN1_PERIOD(20.0),
        .CLKIN2_PERIOD(40.0),
        .CLKFBOUT_MULT(16),
        .CLKOUT0_DIVIDE(8),
        .COMPENSATION("INTERNAL")
    ) pll (
        .CLKIN1(clk), .CLKIN2(clk2), .CLKINSEL(sel), .CLKFBIN(fb), .RST(1'b0), .REL(1'b0),
        .DCLK(1'b0), .DEN(1'b0), .DWE(1'b0), .DADDR(5'd0), .DI(16'd0),
        .CLKFBOUT(fb), .CLKOUT0(o0), .LOCKED(locked)
    );
    wire g0;
    BUFG bufg_0 (.I(o0), .O(g0));
    reg [26:0] n = 0;
    always @(posedge g0) n <= n + 1'b1;
    assign led = {n[26:20], locked};
endmodule
