// Mojo V3 blinky: the 8 LEDs show the top bits of a free-running counter.
// rst_n is the Mojo's reset button (active low).
module blinky (
    input  wire       clk,    // 50 MHz
    input  wire       rst_n,
    output wire [7:0] led
);
    reg [31:0] counter = 0;

    always @(posedge clk) begin
        if (!rst_n)
            counter <= 0;
        else
            counter <= counter + 1;
    end

    assign led = counter[31:24];
endmodule
